//! The loop, driven against a host that answers from a script.
//!
//! Every test here is one of the paths that is awkward to provoke out of a real
//! server: a refusal, a declined confirmation, a reply that will not parse, a
//! model that narrates instead of acting, a step limit. That is what the six
//! traits in `host.rs` are for, and this file is the reason they exist.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::cancel::Cancel;
use super::files::{FileOpError, FilePlan, FileRequest, FileRunner};
use super::host::{AgentConfig, Asker, Bytes, Emitter, Executor, Judge, Stores};
use super::llm::{Completion, CompletionRequest, LlmError, LlmFailure, LlmProvider};
use super::moves::{TransferOpError, TransferPlan, TransferRequest};
use super::policy::command::{classify_command, PolicyResult};
use super::policy::path::{classify_path, PathPolicyResult};
use super::policy::risk::RiskReason;
use super::session::AgentSession;
use super::store::memory::{AppendOutcome, AppendResult, RemoveOutcome};
use super::store::skill::SkillRead;
use super::types::{
    AgentEvent, AgentMode, CommandResult, FileEncoding, FileOpKind, MemoryScope, SkillOutcome,
};

/// A model that hands back a scripted reply per step.
struct Script {
    replies: Mutex<Vec<Result<String, LlmError>>>,
    asked: AtomicUsize,
}

impl Script {
    fn new(replies: Vec<&str>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into_iter().map(|text| Ok(text.to_string())).collect()),
            asked: AtomicUsize::new(0),
        })
    }
    fn failing(error: LlmError) -> Arc<Self> {
        Arc::new(Self { replies: Mutex::new(vec![Err(error)]), asked: AtomicUsize::new(0) })
    }
}

impl LlmProvider for Script {
    fn id(&self) -> &str {
        "script"
    }
    fn describe(&self) -> String {
        "a scripted model".into()
    }
    fn complete<'a>(&'a self, _request: CompletionRequest) -> Completion<'a> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let mut replies = self.replies.lock().unwrap();
        let next = if replies.is_empty() {
            // A script that runs out means the loop went further than the test
            // expected, which is worth failing on rather than hanging.
            Err(LlmError::new(LlmFailure::Unknown, "the script ran out"))
        } else {
            replies.remove(0)
        };
        Box::pin(async move { next })
    }
}

#[derive(Default)]
struct Answers {
    /// What `execute` hands back, in order. The default is a clean exit.
    results: Mutex<Vec<CommandResult>>,
    confirm_command: bool,
    confirm_file: bool,
    mode: AgentMode,
    memory_enabled: bool,
    skills_enabled: bool,
    trusted: bool,
    remember: Option<AppendOutcome>,
    forget: Option<RemoveOutcome>,
    skill: Option<SkillRead>,
    plan_file_error: Option<String>,
    plan_transfer_error: Option<String>,
}

struct Fake {
    answers: Answers,
    events: Mutex<Vec<AgentEvent>>,
    commands: Mutex<Vec<String>>,
}

impl Fake {
    fn new(answers: Answers) -> Arc<Self> {
        Arc::new(Self {
            answers,
            events: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
        })
    }

    fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }

    /// The event tags in order, which is what most of these tests assert on.
    fn tags(&self) -> Vec<String> {
        self.events()
            .iter()
            .map(|event| {
                serde_json::to_value(event).unwrap()["type"].as_str().unwrap_or("?").to_string()
            })
            .collect()
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }
}

fn permissive() -> Answers {
    Answers {
        confirm_command: true,
        confirm_file: true,
        mode: AgentMode::Ask,
        memory_enabled: true,
        skills_enabled: true,
        ..Default::default()
    }
}

impl Judge for Fake {
    fn classify(&self, command: &str) -> PolicyResult {
        classify_command(command, &[])
    }
    fn classify_path(&self, path: &str) -> PathPolicyResult {
        classify_path(path)
    }
    fn describe_risk(&self, reasons: &[RiskReason]) -> Vec<String> {
        reasons.iter().map(|reason| format!("<{}>", reason.key())).collect()
    }
}

impl Executor for Fake {
    async fn execute(&self, command: &str, _cancel: &Cancel) -> CommandResult {
        self.commands.lock().unwrap().push(command.to_string());
        let mut results = self.answers.results.lock().unwrap();
        if results.is_empty() {
            CommandResult { output: "ok".into(), ..Default::default() }
        } else {
            results.remove(0)
        }
    }
}

impl Asker for Fake {
    fn mode(&self) -> AgentMode {
        self.answers.mode
    }
    async fn confirm_command(&self, _command: &str, _why: &str, _policy: &PolicyResult) -> bool {
        self.answers.confirm_command
    }
    async fn confirm_file(&self, _plan: &FilePlan, _why: &str) -> bool {
        self.answers.confirm_file
    }
}

impl Bytes for Fake {
    async fn plan_file(
        &self,
        request: FileRequest,
        _cancel: &Cancel,
    ) -> Result<FilePlan, FileOpError> {
        if let Some(message) = &self.answers.plan_file_error {
            return Err(FileOpError(message.clone()));
        }
        Ok(FilePlan {
            kind: request.kind,
            path: request.path,
            exists: true,
            bytes: 10,
            preview: request.content.clone().unwrap_or_default(),
            before: request.old_text,
            after: request.new_text,
            line: Some(1),
            payload: request.content.unwrap_or_default(),
            encoding: FileEncoding::Utf8,
        })
    }
    async fn apply_file(&self, plan: &FilePlan, _cancel: &Cancel) -> CommandResult {
        CommandResult { output: format!("wrote {}", plan.path), ..Default::default() }
    }
    async fn plan_transfer(
        &self,
        request: TransferRequest,
        _cancel: &Cancel,
    ) -> Result<TransferPlan, TransferOpError> {
        if let Some(message) = &self.answers.plan_transfer_error {
            return Err(TransferOpError(message.clone()));
        }
        Ok(TransferPlan {
            kind: request.kind,
            roots: request
                .paths
                .into_iter()
                .map(|path| super::moves::TransferRoot { path, is_directory: false })
                .collect(),
            target: request.to,
        })
    }
    async fn run_transfer(&self, _plan: &TransferPlan, _cancel: &Cancel) -> CommandResult {
        CommandResult { output: "moved".into(), ..Default::default() }
    }
}

impl Stores for Fake {
    fn memory_enabled(&self) -> bool {
        self.answers.memory_enabled
    }
    async fn remember(&self, _scope: MemoryScope, _text: &str) -> AppendResult {
        AppendResult {
            outcome: self.answers.remember.unwrap_or(AppendOutcome::Ok),
            token: Some("t1".into()),
        }
    }
    async fn forget(&self, _scope: MemoryScope, _text: &str) -> RemoveOutcome {
        self.answers.forget.unwrap_or(RemoveOutcome::Ok)
    }
    fn skills_enabled(&self) -> bool {
        self.answers.skills_enabled
    }
    async fn load_skill(&self, _id: &str, _file: Option<&str>) -> SkillRead {
        self.answers.skill.clone().unwrap_or(SkillRead {
            outcome: SkillOutcome::Ok,
            content: Some("the procedure".into()),
            file: Some("SKILL.md".into()),
            truncated: false,
        })
    }
    async fn is_trusted(&self, _path: &str) -> bool {
        self.answers.trusted
    }
}

impl Emitter for Fake {
    fn emit(&self, event: AgentEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// A runner nothing in these tests uses, so `files.rs` need not be dragged in.
struct NoRunner;
impl FileRunner for NoRunner {
    fn run<'a>(
        &'a self,
        _command: String,
        _options: super::shell::RunOptions,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
        Box::pin(async { CommandResult::default() })
    }
}

fn session(host: Arc<Fake>, script: Arc<Script>) -> AgentSession<Fake> {
    AgentSession::new(host, AgentConfig::new(script))
}

fn reply(text: &str) -> String {
    text.to_string()
}

#[tokio::test]
async fn a_say_ends_the_task_with_the_answer() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![r#"{"action":"say","text":"hello there"}"#]);
    session(host.clone(), script).send("hi").await;

    assert_eq!(host.tags(), ["thinking", "context", "usage", "reply", "idle"]);
    assert!(matches!(&host.events()[3], AgentEvent::Reply { text } if text == "hello there"));
}

#[tokio::test]
async fn plain_prose_before_anything_has_run_is_the_answer() {
    let host = Fake::new(permissive());
    let script = Script::new(vec!["The disk is nearly full."]);
    session(host.clone(), script).send("how is the disk?").await;

    assert!(host.tags().contains(&"reply".to_string()));
    assert!(!host.tags().contains(&"error".to_string()));
}

#[tokio::test]
async fn a_command_runs_and_its_result_comes_back() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"ls -la","why":"look"}"#,
        r#"{"action":"done","summary":"had a look"}"#,
    ]);
    session(host.clone(), script).send("look around").await;

    assert_eq!(host.commands(), vec!["ls -la"]);
    let tags = host.tags();
    assert!(tags.contains(&"command".to_string()));
    assert!(tags.contains(&"result".to_string()));
    assert!(tags.contains(&"summary".to_string()));
}

#[tokio::test]
async fn a_destructive_command_is_refused_and_the_task_carries_on() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"rm -rf /","why":"clean up"}"#,
        r#"{"action":"done","summary":"stopped"}"#,
    ]);
    session(host.clone(), script).send("clean up").await;

    assert!(host.commands().is_empty(), "nothing was typed into the terminal");
    let refused = host
        .events()
        .into_iter()
        .find(|event| matches!(event, AgentEvent::Refused { .. }))
        .expect("a refusal");
    assert!(matches!(refused, AgentEvent::Refused { ref reasons, .. }
        if reasons == &vec!["<removesRoot>".to_string()]));
    // The task did not end on the refusal: the next step still ran.
    assert!(host.tags().contains(&"summary".to_string()));
}

#[tokio::test]
async fn a_declined_confirmation_runs_nothing_and_says_so() {
    let host = Fake::new(Answers { confirm_command: false, ..permissive() });
    let script = Script::new(vec![
        r#"{"action":"run","command":"systemctl restart nginx","why":"restart"}"#,
        r#"{"action":"done","summary":"left alone"}"#,
    ]);
    session(host.clone(), script).send("restart nginx").await;

    assert!(host.commands().is_empty());
    assert!(host.tags().contains(&"declined".to_string()));
}

#[tokio::test]
async fn auto_mode_answers_the_confirmation_but_not_the_refusal() {
    let host = Fake::new(Answers { confirm_command: false, mode: AgentMode::Auto, ..permissive() });
    let script = Script::new(vec![
        r#"{"action":"run","command":"systemctl restart nginx","why":"restart"}"#,
        r#"{"action":"run","command":"rm -rf /","why":"clean"}"#,
        r#"{"action":"done","summary":"done"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    assert_eq!(host.commands(), vec!["systemctl restart nginx"], "the confirmed one ran unasked");
    let marked = host.events().into_iter().any(|event| {
        matches!(event, AgentEvent::Command { unconfirmed: Some(super::types::Unconfirmed::Auto), .. })
    });
    assert!(marked, "the card says it was not confirmed");
    assert!(host.tags().contains(&"refused".to_string()), "the refusal still refuses");
}

#[tokio::test]
async fn a_reply_that_will_not_parse_is_nudged_once_and_then_given_up_on() {
    let host = Fake::new(permissive());
    // An `"action"` key inside something that does not parse: not prose, not an action.
    let broken = r#"{"action":"say","text":"he said "hi" loudly"}"#;
    let script = Script::new(vec![reply(broken).as_str(), broken]);
    session(host.clone(), script).send("hi").await;

    let error = host
        .events()
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::Error { code, .. } => code,
            _ => None,
        })
        .expect("an error");
    assert_eq!(error, "protocol");
}

#[tokio::test]
async fn a_reply_whose_first_action_is_broken_runs_nothing_at_all() {
    let host = Fake::new(permissive());
    let broken_then_good = concat!(
        r#"{"action":"say","text":"he said "hi""} "#,
        r#"{"action":"run","command":"rm -rf /tmp/everything"}"#
    );
    let script = Script::new(vec![broken_then_good, r#"{"action":"done","summary":"ok"}"#]);
    session(host.clone(), script).send("go").await;

    assert!(host.commands().is_empty(), "the second action must not run in the first's place");
}

#[tokio::test]
async fn narration_after_work_has_started_asks_for_the_action_once() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"ls","why":"look"}"#,
        "Now I will read the config file.",
        "Now I will read the config file.",
    ]);
    session(host.clone(), script).send("go").await;

    let replies = host
        .events()
        .into_iter()
        .filter(|event| matches!(event, AgentEvent::Reply { .. }))
        .count();
    assert_eq!(replies, 2, "both narrations are shown");
    // The second one ends the task rather than nudging again.
    assert_eq!(host.tags().last().unwrap(), "idle");
}

#[tokio::test]
async fn a_reply_carrying_several_actions_runs_the_first_and_says_so() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        concat!(r#"{"action":"run","command":"ls"} "#, r#"{"action":"run","command":"df"}"#),
        r#"{"action":"done","summary":"ok"}"#,
    ]);
    let session = session(host.clone(), script);
    session.send("go").await;

    assert_eq!(host.commands(), vec!["ls"]);
    let notice = session
        .history()
        .into_iter()
        .find(|message| message.content.contains("carried 2 actions"))
        .expect("the dropped-actions notice");
    assert!(notice.content.contains("only the first was carried out"));
}

#[tokio::test]
async fn a_file_action_is_planned_shown_confirmed_and_applied() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"write","path":"/tmp/x","content":"hello","why":"because"}"#,
        r#"{"action":"done","summary":"written"}"#,
    ]);
    session(host.clone(), script).send("write it").await;

    let tags = host.tags();
    let at = |name: &str| tags.iter().position(|tag| tag == name).unwrap();
    assert!(at("command") < at("file"), "the card is drawn before the change is shown");
    assert!(at("file") < at("result"));
}

#[tokio::test]
async fn a_write_to_a_device_is_refused_before_the_machine_is_touched() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"write","path":"/dev/sda","content":"x"}"#,
        r#"{"action":"done","summary":"stopped"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    assert!(host.tags().contains(&"refused".to_string()));
    assert!(!host.tags().contains(&"file".to_string()), "nothing was planned");
}

#[tokio::test]
async fn trust_mode_skips_the_dialog_only_for_a_trusted_directory() {
    let host = Fake::new(Answers {
        confirm_file: false,
        mode: AgentMode::Trust,
        trusted: true,
        ..permissive()
    });
    let script = Script::new(vec![
        r#"{"action":"write","path":"/srv/app/x","content":"hello"}"#,
        r#"{"action":"done","summary":"ok"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    let marked = host.events().into_iter().any(|event| {
        matches!(
            event,
            AgentEvent::Command { unconfirmed: Some(super::types::Unconfirmed::Trusted), .. }
        )
    });
    assert!(marked);
    assert!(!host.tags().contains(&"declined".to_string()), "it went through unasked");
}

#[tokio::test]
async fn a_plan_that_fails_reports_itself_in_the_card_it_opened() {
    let host = Fake::new(Answers {
        plan_file_error: Some("that text does not appear".into()),
        ..permissive()
    });
    let script = Script::new(vec![
        r#"{"action":"edit","path":"/tmp/x","old":"a","new":"b"}"#,
        r#"{"action":"done","summary":"gave up"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    let failed = host.events().into_iter().any(|event| {
        matches!(event, AgentEvent::Result { exit_code: 1, ref output, .. }
            if output.contains("does not appear"))
    });
    assert!(failed);
}

#[tokio::test]
async fn a_remembered_line_is_reported_with_an_undo_token() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"remember","scope":"global","text":"nginx is at /opt"}"#,
        r#"{"action":"done","summary":"noted"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    let remembered = host
        .events()
        .into_iter()
        .find(|event| matches!(event, AgentEvent::Memory { .. }))
        .expect("a memory card");
    assert!(matches!(remembered, AgentEvent::Memory { token: Some(ref token), .. } if token == "t1"));
}

#[tokio::test]
async fn a_transcribed_paragraph_is_refused_as_a_fact() {
    let host = Fake::new(permissive());
    let long = "x".repeat(super::prompt::MAX_FACT_CHARS + 1);
    let action = format!(r#"{{"action":"remember","text":"{long}"}}"#);
    let script = Script::new(vec![&action, r#"{"action":"done","summary":"ok"}"#]);
    session(host.clone(), script).send("go").await;

    let oversize = host.events().into_iter().any(|event| {
        matches!(event, AgentEvent::Memory { outcome: super::types::MemoryOutcome::Oversize, .. })
    });
    assert!(oversize);
}

#[tokio::test]
async fn memory_switched_off_answers_the_action_rather_than_writing() {
    let host = Fake::new(Answers { memory_enabled: false, ..permissive() });
    let script = Script::new(vec![
        r#"{"action":"remember","text":"a fact"}"#,
        r#"{"action":"done","summary":"ok"}"#,
    ]);
    let session = session(host.clone(), script);
    session.send("go").await;

    assert!(!host.tags().contains(&"memory".to_string()));
    assert!(session
        .history()
        .iter()
        .any(|message| message.content.contains("Memory is switched off")));
}

#[tokio::test]
async fn a_skill_is_read_once_and_the_second_load_is_told_to_scroll_up() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"skill","name":"deploy"}"#,
        r#"{"action":"skill","name":"deploy"}"#,
        r#"{"action":"done","summary":"ok"}"#,
    ]);
    let session = session(host.clone(), script);
    session.send("go").await;

    let outcomes: Vec<SkillOutcome> = host
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::Skill { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes, vec![SkillOutcome::Ok, SkillOutcome::Repeat]);
    assert!(session
        .history()
        .iter()
        .any(|message| message.content.starts_with("SKILL deploy (SKILL.md):")));
}

#[tokio::test]
async fn a_transfer_runs_without_a_confirmation() {
    let host = Fake::new(Answers { confirm_command: false, ..permissive() });
    let script = Script::new(vec![
        r#"{"action":"download","path":"/var/log/a","to":"/tmp"}"#,
        r#"{"action":"done","summary":"moved"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    assert!(host.tags().contains(&"transfer".to_string()));
    assert!(!host.tags().contains(&"declined".to_string()));
}

#[tokio::test]
async fn a_transfer_into_a_device_is_refused() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"download","path":"/var/log/a","to":"/dev"}"#,
        r#"{"action":"done","summary":"stopped"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    assert!(host.tags().contains(&"refused".to_string()));
    assert!(!host.tags().contains(&"transfer".to_string()));
}

#[tokio::test]
async fn the_step_limit_ends_the_task_and_says_so() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"ls"}"#,
        r#"{"action":"run","command":"ls"}"#,
        r#"{"action":"run","command":"ls"}"#,
    ]);
    let session = session(host.clone(), script);
    session.configure(|config| config.max_steps = 2);
    session.send("go").await;

    assert_eq!(host.commands().len(), 2);
    assert!(host.events().into_iter().any(|event| matches!(
        event,
        AgentEvent::StepLimit { steps: 2 }
    )));
}

#[tokio::test]
async fn stopping_ends_the_task_with_stopped_rather_than_an_error() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![r#"{"action":"run","command":"ls"}"#; 3]);
    let session = Arc::new(session(host.clone(), script));

    let running = session.clone();
    let task = tokio::spawn(async move { running.send("go").await });
    // The loop is asynchronous, so it is stopped once it has had a chance to start.
    tokio::task::yield_now().await;
    session.stop();
    task.await.unwrap();

    assert_eq!(host.tags().last().unwrap(), "idle");
}

#[tokio::test]
async fn a_model_failure_is_reported_with_the_code_the_panel_translates() {
    let host = Fake::new(permissive());
    let script = Script::failing(LlmError::new(LlmFailure::Unauthorized, "HTTP 401"));
    session(host.clone(), script).send("go").await;

    let code = host
        .events()
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::Error { code, .. } => code,
            _ => None,
        })
        .expect("an error");
    assert_eq!(code, "aiUnauthorized");
}

#[tokio::test]
async fn an_empty_reply_is_asked_again_once() {
    let host = Fake::new(permissive());
    let script = Arc::new(Script {
        replies: Mutex::new(vec![
            Err(LlmError::new(LlmFailure::Empty, "nothing")),
            Ok(r#"{"action":"say","text":"second time lucky"}"#.into()),
        ]),
        asked: AtomicUsize::new(0),
    });
    session(host.clone(), script.clone()).send("go").await;

    assert_eq!(script.asked.load(Ordering::SeqCst), 2);
    assert!(host.events().into_iter().any(|event| matches!(
        event,
        AgentEvent::Retry { ref kind, .. } if kind == "empty"
    )));
}

#[tokio::test]
async fn narration_ahead_of_an_action_is_shown_before_the_card() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        "I'll check the disk first.\n{\"action\":\"run\",\"command\":\"df -h\"}",
        r#"{"action":"done","summary":"ok"}"#,
    ]);
    session(host.clone(), script).send("go").await;

    let tags = host.tags();
    let reply_at = tags.iter().position(|tag| tag == "reply").unwrap();
    let command_at = tags.iter().position(|tag| tag == "command").unwrap();
    assert!(reply_at < command_at);
}

#[tokio::test]
async fn the_oldest_command_output_is_folded_away_once_the_budget_is_passed() {
    let host = Fake::new(permissive());
    // Long enough that there is something outside the last six turns to fold:
    // `trim` never touches those, nor the opening turn.
    let mut replies = vec![r#"{"action":"run","command":"cat big"}"#; 8];
    replies.push(r#"{"action":"done","summary":"ok"}"#);
    let script = Script::new(replies);
    for _ in 0..8 {
        host.answers
            .results
            .lock()
            .unwrap()
            .push(CommandResult { output: "x".repeat(4000), ..Default::default() });
    }

    let session = session(host.clone(), script);
    session.configure(|config| config.context_budget = 2000);
    session.send("go").await;

    // Nothing is asserted about which turn folded -- only that the conversation
    // came back under the budget by dropping output rather than whole turns.
    let history = session.history();
    assert!(
        history.iter().any(|message| message.content.contains("[Output omitted here:")),
        "the oldest output was folded"
    );
    assert!(
        history.iter().any(|message| message.content.contains("EXIT:")),
        "the shape of the task survives -- the exit codes are still there"
    );
    assert_eq!(history[0].content, "go", "the task itself is never folded");
}

#[tokio::test]
async fn a_restored_conversation_knows_which_skills_are_already_in_it() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"skill","name":"deploy"}"#,
        r#"{"action":"done","summary":"ok"}"#,
    ]);
    let session = session(host.clone(), script);
    session.restore(vec![
        super::llm::ChatMessage::user("go"),
        super::llm::ChatMessage::user(&format!(
            "{}\nthe procedure",
            super::prompt::skill_header("deploy", "SKILL.md")
        )),
    ]);
    session.send("again").await;

    let repeated = host.events().into_iter().any(|event| {
        matches!(event, AgentEvent::Skill { outcome: SkillOutcome::Repeat, .. })
    });
    assert!(repeated, "the body already in the conversation was not fetched twice");
}

#[test]
fn the_file_runner_shim_is_only_here_to_keep_the_import_honest() {
    // `NoRunner` exists so `files::FileRunner` is exercised from this file's
    // imports; nothing in the loop takes one directly.
    let _ = NoRunner;
}
