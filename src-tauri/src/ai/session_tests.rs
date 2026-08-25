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
use super::llm::{
    ChatMessage, Completion, CompletionRequest, LlmError, LlmFailure, LlmProvider, Reply, ToolCall,
};
use super::moves::{TransferOpError, TransferPlan, TransferRequest};
use super::policy::command::{classify_command, PolicyResult};
use super::policy::path::{classify_path, PathPolicyResult};
use super::policy::risk::RiskReason;
use super::session::AgentSession;
use super::store::memory::{AppendOutcome, AppendResult};
use super::store::skill::SkillRead;
use super::types::{AgentEvent, AgentMode, CommandResult, FileEncoding, MemoryScope, SkillOutcome};

/// A model that hands back a scripted reply per step.
struct Script {
    replies: Mutex<Vec<Result<Reply, LlmError>>>,
    asked: AtomicUsize,
}

impl Script {
    fn new(replies: Vec<&str>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(
                replies
                    .into_iter()
                    .map(|text| Ok(Reply::text(text)))
                    .collect(),
            ),
            asked: AtomicUsize::new(0),
        })
    }
    fn failing(error: LlmError) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(vec![Err(error)]),
            asked: AtomicUsize::new(0),
        })
    }
    /// Replies built exactly as the test wants them, for the cases where the
    /// difference between the two tracks is the thing under test.
    fn scripted(replies: Vec<Reply>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(replies.into_iter().map(Ok).collect()),
            asked: AtomicUsize::new(0),
        })
    }
    /// Replies made of native tool calls: the other track, step for step.
    ///
    /// Each inner list is one reply, so `vec![vec![a, b], vec![c]]` is a step
    /// that called two tools at once followed by a step that called one. An empty
    /// list is a reply that called nothing, which is how a task ends here.
    fn calling(replies: Vec<Vec<(&str, &str)>>) -> Arc<Self> {
        Arc::new(Self {
            replies: Mutex::new(
                replies
                    .into_iter()
                    .map(|step| {
                        Ok(Reply::calls(
                            step.into_iter()
                                .enumerate()
                                .map(|(index, (name, arguments))| ToolCall {
                                    id: format!("call_{index}"),
                                    name: name.to_string(),
                                    arguments: arguments.to_string(),
                                })
                                .collect(),
                        ))
                    })
                    .collect(),
            ),
            asked: AtomicUsize::new(0),
        })
    }
}

impl LlmProvider for Script {
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
    skill: Option<SkillRead>,
    plan_file_error: Option<String>,
    plan_transfer_error: Option<String>,
}

struct Fake {
    answers: Answers,
    events: Mutex<Vec<AgentEvent>>,
    commands: Mutex<Vec<String>>,
    checkpoints: Mutex<Vec<Vec<ChatMessage>>>,
}

impl Fake {
    fn new(answers: Answers) -> Arc<Self> {
        Arc::new(Self {
            answers,
            events: Mutex::new(Vec::new()),
            commands: Mutex::new(Vec::new()),
            checkpoints: Mutex::new(Vec::new()),
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
                serde_json::to_value(event).unwrap()["type"]
                    .as_str()
                    .unwrap_or("?")
                    .to_string()
            })
            .collect()
    }

    fn commands(&self) -> Vec<String> {
        self.commands.lock().unwrap().clone()
    }

    fn checkpoints(&self) -> Vec<Vec<ChatMessage>> {
        self.checkpoints.lock().unwrap().clone()
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
        reasons
            .iter()
            .map(|reason| format!("<{}>", reason.key()))
            .collect()
    }
}

impl Executor for Fake {
    async fn execute(&self, command: &str, _cancel: &Cancel) -> CommandResult {
        self.commands.lock().unwrap().push(command.to_string());
        let mut results = self.answers.results.lock().unwrap();
        if results.is_empty() {
            CommandResult {
                output: "ok".into(),
                ..Default::default()
            }
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
        CommandResult {
            output: format!("wrote {}", plan.path),
            ..Default::default()
        }
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
                .map(|path| super::moves::TransferRoot {
                    path,
                    is_directory: false,
                })
                .collect(),
            target: request.to,
        })
    }
    async fn run_transfer(&self, _plan: &TransferPlan, _cancel: &Cancel) -> CommandResult {
        CommandResult {
            output: "moved".into(),
            ..Default::default()
        }
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
    fn checkpoint(&self, messages: &[ChatMessage]) {
        self.checkpoints.lock().unwrap().push(messages.to_vec());
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
async fn prose_alone_is_the_answer_and_ends_the_task() {
    let host = Fake::new(permissive());
    let script = Script::new(vec!["hello there"]);
    session(host.clone(), script).send("hi").await;

    assert_eq!(
        host.tags(),
        ["thinking", "context", "usage", "reply", "idle"]
    );
    assert!(matches!(&host.events()[3], AgentEvent::Reply { text } if text == "hello there"));
}

#[tokio::test]
async fn history_is_checkpointed_before_a_task_finishes() {
    let host = Fake::new(permissive());
    let script = Script::new(vec!["done"]);
    session(host.clone(), script).send("start").await;

    let checkpoints = host.checkpoints();
    assert!(checkpoints
        .iter()
        .any(|history| { history.len() == 1 && history[0] == ChatMessage::user("start") }));
    assert!(checkpoints
        .iter()
        .any(|history| { history.len() == 2 && history[1] == ChatMessage::assistant("done") }));
}

/// The commonest false alarm the old scan had: an answer that shows the user
/// somebody else's JSON. Every one of those cost the user their answer.
#[tokio::test]
async fn an_answer_quoting_json_is_still_an_answer() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        "Post this:
```json
{\"action\":\"run\",\"command\":\"rm -rf /\"}
```
and it returns 202.",
    ]);
    session(host.clone(), script)
        .send("what does the body look like?")
        .await;

    assert!(
        host.commands().is_empty(),
        "an example must not be carried out"
    );
    assert_eq!(
        host.tags(),
        ["thinking", "context", "usage", "reply", "idle"]
    );
}

#[tokio::test]
async fn a_command_runs_and_its_result_comes_back() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"ls -la","why":"look"}"#,
        "had a look",
    ]);
    session(host.clone(), script).send("look around").await;

    assert_eq!(host.commands(), vec!["ls -la"]);
    let tags = host.tags();
    assert!(tags.contains(&"command".to_string()));
    assert!(tags.contains(&"result".to_string()));
    assert!(tags.contains(&"reply".to_string()));
}

#[tokio::test]
async fn a_destructive_command_is_refused_and_the_task_carries_on() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"rm -rf /","why":"clean up"}"#,
        "stopped",
    ]);
    session(host.clone(), script).send("clean up").await;

    assert!(
        host.commands().is_empty(),
        "nothing was typed into the terminal"
    );
    let refused = host
        .events()
        .into_iter()
        .find(|event| matches!(event, AgentEvent::Refused { .. }))
        .expect("a refusal");
    assert!(matches!(refused, AgentEvent::Refused { ref reasons, .. }
        if reasons == &vec!["<removesRoot>".to_string()]));
    // The task did not end on the refusal: the next step still ran.
    assert!(host.tags().contains(&"reply".to_string()));
}

#[tokio::test]
async fn a_declined_confirmation_runs_nothing_and_says_so() {
    let host = Fake::new(Answers {
        confirm_command: false,
        ..permissive()
    });
    let script = Script::new(vec![
        r#"{"action":"run","command":"systemctl restart nginx","why":"restart"}"#,
        "left alone",
    ]);
    session(host.clone(), script).send("restart nginx").await;

    assert!(host.commands().is_empty());
    assert!(host.tags().contains(&"declined".to_string()));
}

#[tokio::test]
async fn auto_mode_answers_the_confirmation_but_not_the_refusal() {
    let host = Fake::new(Answers {
        confirm_command: false,
        mode: AgentMode::Auto,
        ..permissive()
    });
    let script = Script::new(vec![
        r#"{"action":"run","command":"systemctl restart nginx","why":"restart"}"#,
        r#"{"action":"run","command":"rm -rf /","why":"clean"}"#,
        "one ran, one did not",
    ]);
    session(host.clone(), script).send("go").await;

    assert_eq!(
        host.commands(),
        vec!["systemctl restart nginx"],
        "the confirmed one ran unasked"
    );
    let marked = host.events().into_iter().any(|event| {
        matches!(
            event,
            AgentEvent::Command {
                unconfirmed: Some(super::types::Unconfirmed::Auto),
                ..
            }
        )
    });
    assert!(marked, "the card says it was not confirmed");
    assert!(
        host.tags().contains(&"refused".to_string()),
        "the refusal still refuses"
    );
}

#[tokio::test]
async fn a_reply_that_will_not_parse_is_nudged_once_and_then_given_up_on() {
    let host = Fake::new(permissive());
    // A verb this build has, inside something that will not parse: an action the
    // model believes it sent, not prose and not an example.
    let broken = r#"{"action":"run","command":"echo "hi" loudly"}"#;
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

/// Only the object at the END is the instruction. One earlier in the reply is
/// the model showing its work, and running it would carry out an example.
#[tokio::test]
async fn only_the_object_at_the_end_of_a_reply_is_carried_out() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        concat!(
            r#"You could write {"action":"run","command":"rm -rf /tmp/everything"} "#,
            r#"but I will look first. {"action":"run","command":"ls /tmp"}"#
        ),
        "had a look",
    ]);
    session(host.clone(), script).send("go").await;

    assert_eq!(host.commands(), vec!["ls /tmp"]);
}

/// Narration with nothing after it is the answer, on both tracks. It used to be
/// nudged on this one, which cost a request and drew a second card under an
/// answer the user had already read.
#[tokio::test]
async fn narration_after_work_has_started_ends_the_task() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"run","command":"ls","why":"look"}"#,
        "Now I will read the config file.",
    ]);
    let session = session(host.clone(), script);
    session.send("go").await;

    let replies = host
        .events()
        .into_iter()
        .filter(|event| matches!(event, AgentEvent::Reply { .. }))
        .count();
    assert_eq!(replies, 1);
    assert_eq!(host.tags().last().unwrap(), "idle");
    assert!(
        !session
            .history()
            .into_iter()
            .any(|message| message.content.contains("NOTHING")),
        "the model was not told its answer had gone missing",
    );
}

#[tokio::test]
async fn a_file_action_is_planned_shown_confirmed_and_applied() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"write","path":"/tmp/x","content":"hello","why":"because"}"#,
        "written",
    ]);
    session(host.clone(), script).send("write it").await;

    let tags = host.tags();
    let at = |name: &str| tags.iter().position(|tag| tag == name).unwrap();
    assert!(
        at("command") < at("file"),
        "the card is drawn before the change is shown"
    );
    assert!(at("file") < at("result"));
}

#[tokio::test]
async fn a_write_to_a_device_is_refused_before_the_machine_is_touched() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        r#"{"action":"write","path":"/dev/sda","content":"x"}"#,
        "stopped",
    ]);
    session(host.clone(), script).send("go").await;

    assert!(host.tags().contains(&"refused".to_string()));
    assert!(
        !host.tags().contains(&"file".to_string()),
        "nothing was planned"
    );
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
        "ok",
    ]);
    session(host.clone(), script).send("go").await;

    let marked = host.events().into_iter().any(|event| {
        matches!(
            event,
            AgentEvent::Command {
                unconfirmed: Some(super::types::Unconfirmed::Trusted),
                ..
            }
        )
    });
    assert!(marked);
    assert!(
        !host.tags().contains(&"declined".to_string()),
        "it went through unasked"
    );
}

#[tokio::test]
async fn a_plan_that_fails_reports_itself_in_the_card_it_opened() {
    let host = Fake::new(Answers {
        plan_file_error: Some("that text does not appear".into()),
        ..permissive()
    });
    let script = Script::new(vec![
        r#"{"action":"edit","path":"/tmp/x","old":"a","new":"b"}"#,
        "gave up",
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
        "noted",
    ]);
    session(host.clone(), script).send("go").await;

    let remembered = host
        .events()
        .into_iter()
        .find(|event| matches!(event, AgentEvent::Memory { .. }))
        .expect("a memory card");
    assert!(
        matches!(remembered, AgentEvent::Memory { token: Some(ref token), .. } if token == "t1")
    );
}

#[tokio::test]
async fn a_transcribed_paragraph_is_refused_as_a_fact() {
    let host = Fake::new(permissive());
    let long = "x".repeat(super::prompt::MAX_FACT_CHARS + 1);
    let action = format!(r#"{{"action":"remember","text":"{long}"}}"#);
    let script = Script::new(vec![&action, "ok"]);
    session(host.clone(), script).send("go").await;

    let oversize = host.events().into_iter().any(|event| {
        matches!(
            event,
            AgentEvent::Memory {
                outcome: super::types::MemoryOutcome::Oversize,
                ..
            }
        )
    });
    assert!(oversize);
}

#[tokio::test]
async fn memory_switched_off_answers_the_action_rather_than_writing() {
    let host = Fake::new(Answers {
        memory_enabled: false,
        ..permissive()
    });
    let script = Script::new(vec![r#"{"action":"remember","text":"a fact"}"#, "ok"]);
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
        "ok",
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
    let host = Fake::new(Answers {
        confirm_command: false,
        ..permissive()
    });
    let script = Script::new(vec![
        r#"{"action":"download","path":"/var/log/a","to":"/tmp"}"#,
        "moved",
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
        "stopped",
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
    assert!(host
        .events()
        .into_iter()
        .any(|event| matches!(event, AgentEvent::StepLimit { steps: 2 })));
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
        "ok",
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
    replies.push("ok");
    let script = Script::new(replies);
    for _ in 0..8 {
        host.answers.results.lock().unwrap().push(CommandResult {
            output: "x".repeat(4000),
            ..Default::default()
        });
    }

    let session = session(host.clone(), script);
    session.configure(|config| config.context_budget = 2000);
    session.send("go").await;

    // Nothing is asserted about which turn folded -- only that the conversation
    // came back under the budget by dropping output rather than whole turns.
    let history = session.history();
    assert!(
        history
            .iter()
            .any(|message| message.content.contains("[Output omitted here:")),
        "the oldest output was folded"
    );
    assert!(
        history
            .iter()
            .any(|message| message.content.contains("EXIT:")),
        "the shape of the task survives -- the exit codes are still there"
    );
    assert_eq!(history[0].content, "go", "the task itself is never folded");
}

#[tokio::test]
async fn a_short_command_output_is_not_expanded_while_trimming() {
    let host = Fake::new(permissive());
    // Empty results are shorter than the explanatory folding placeholder. Once
    // enough turns cross the budget, trimming must skip them instead of growing
    // the context (and, in debug builds, subtracting with overflow).
    let mut replies = vec![r#"{"action":"run","command":"true"}"#; 8];
    replies.push("ok");
    let script = Script::new(replies);

    let session = session(host, script);
    session.configure(|config| config.context_budget = 200);
    session.send("go").await;

    let history = session.history();
    assert_eq!(
        history.last().map(|message| message.content.as_str()),
        Some("ok")
    );
    assert!(
        history
            .iter()
            .all(|message| !message.content.contains("[Output omitted here:")),
        "a fold that would make a short result longer must be skipped"
    );
}

#[tokio::test]
async fn a_restored_conversation_knows_which_skills_are_already_in_it() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![r#"{"action":"skill","name":"deploy"}"#, "ok"]);
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
        matches!(
            event,
            AgentEvent::Skill {
                outcome: SkillOutcome::Repeat,
                ..
            }
        )
    });
    assert!(
        repeated,
        "the body already in the conversation was not fetched twice"
    );
}

#[test]
fn the_file_runner_shim_is_only_here_to_keep_the_import_honest() {
    // `NoRunner` exists so `files::FileRunner` is exercised from this file's
    // imports; nothing in the loop takes one directly.
    let _ = NoRunner;
}

// ------------------------------------------------------ the tool-call track ---

#[tokio::test]
async fn a_tool_call_runs_the_same_action_the_json_object_would_have() {
    let host = Fake::new(permissive());
    let script = Script::calling(vec![
        vec![("run", r#"{"command":"df -h","why":"disk"}"#)],
        vec![],
    ]);
    session(host.clone(), script).send("check the disk").await;

    assert_eq!(host.commands(), ["df -h"]);
    assert!(host.tags().contains(&"reply".to_string()));
}

/// The payoff. Three read-only probes are one model mistake on the JSON track and
/// the endpoint's own parallel-call feature on this one, so all three run in the
/// step that asked for them rather than costing three round trips.
#[tokio::test]
async fn parallel_calls_all_run_in_one_step() {
    let host = Fake::new(permissive());
    let script = Script::calling(vec![
        vec![
            ("run", r#"{"command":"df -h","why":"disk"}"#),
            ("run", r#"{"command":"free -m","why":"memory"}"#),
            ("run", r#"{"command":"uptime","why":"load"}"#),
        ],
        vec![],
    ]);
    let script_asked = script.clone();
    session(host.clone(), script).send("look around").await;

    assert_eq!(host.commands(), ["df -h", "free -m", "uptime"]);
    // Two requests, not four: the batch, and the step that ended it.
    assert_eq!(script_asked.asked.load(Ordering::SeqCst), 2);
}

/// Every action still goes through the same gate. A batch is a batch of
/// confirmations too, and a refused command in one does not carry the rest with
/// it -- but it does not run either.
#[tokio::test]
async fn a_batch_does_not_smuggle_a_command_past_the_confirmation() {
    let host = Fake::new(Answers {
        confirm_command: false,
        ..permissive()
    });
    let script = Script::calling(vec![
        vec![
            ("run", r#"{"command":"rm -rf /var/data","why":"cleanup"}"#),
            ("run", r#"{"command":"rm -rf /var/other","why":"cleanup"}"#),
        ],
        vec![],
    ]);
    session(host.clone(), script).send("clean up").await;

    assert!(
        host.commands().is_empty(),
        "neither may run: {:?}",
        host.commands()
    );
}

/// Native results remain one tool message per call, paired by id.
#[tokio::test]
async fn a_batch_reports_its_results_as_one_labelled_turn() {
    let host = Fake::new(permissive());
    let script = Script::calling(vec![
        vec![
            ("run", r#"{"command":"df -h","why":"disk"}"#),
            ("run", r#"{"command":"free -m","why":"memory"}"#),
        ],
        vec![],
    ]);
    let session = session(host.clone(), script);
    session.send("look").await;

    let messages = session.messages_for_test();
    let observations: Vec<_> = messages
        .iter()
        .filter(|message| message.role == crate::ai::llm::ChatRole::Tool)
        .collect();
    assert_eq!(
        observations.len(),
        2,
        "observations were: {observations:#?}"
    );
    let calls = messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .expect("the assistant called both tools");
    assert_eq!(
        observations[0].tool_call_id.as_deref(),
        Some(calls.tool_calls[0].id.as_str())
    );
    assert_eq!(
        observations[1].tool_call_id.as_deref(),
        Some(calls.tool_calls[1].id.as_str())
    );
    assert!(!observations[0].content.is_empty());
    assert!(!observations[1].content.is_empty());
}

/// Native calls are durable as calls, not flattened into assistant prose.
#[tokio::test]
async fn a_call_is_written_into_the_history_as_the_object_it_stands_for() {
    let host = Fake::new(permissive());
    let script = Script::calling(vec![
        vec![("run", r#"{"command":"uptime","why":"load"}"#)],
        vec![],
    ]);
    let session = session(host.clone(), script);
    session.send("go").await;

    let messages = session.messages_for_test();
    let spoken = messages
        .iter()
        .find(|message| message.role == crate::ai::llm::ChatRole::Assistant)
        .expect("the model spoke");
    assert!(spoken.content.is_empty());
    assert_eq!(spoken.tool_calls.len(), 1);
    assert_eq!(spoken.tool_calls[0].name, "run");
    assert!(spoken.tool_calls[0].arguments.contains("uptime"));
    let result = messages
        .iter()
        .find(|message| message.role == crate::ai::llm::ChatRole::Tool)
        .expect("the tool answered");
    assert_eq!(
        result.tool_call_id.as_deref(),
        Some(spoken.tool_calls[0].id.as_str())
    );
}

/// The JSON track carries one action per step, and it is the last object in the
/// reply. Parallel calls are a feature of the other track, where the endpoint
/// constrains each one; three objects run into each other in a stream of text.
#[tokio::test]
async fn a_json_reply_with_three_objects_runs_only_the_last() {
    let host = Fake::new(permissive());
    let script = Script::new(vec![
        concat!(
            r#"{"action":"run","command":"first","why":"a"}"#,
            r#"{"action":"run","command":"second","why":"b"}"#,
            r#"{"action":"run","command":"third","why":"c"}"#
        ),
        "ok",
    ]);
    session(host.clone(), script).send("go").await;

    assert_eq!(host.commands(), ["third"]);
}

/// The rule the JSON track has, kept here for the same reason: if the FIRST thing
/// the model asked for could not be read, nothing runs. A model that said "tell
/// them this, then check that" and lost the telling must not have the checking
/// run in silence.
#[tokio::test]
async fn a_batch_whose_first_call_is_unreadable_runs_nothing() {
    let host = Fake::new(permissive());
    let script = Script::calling(vec![
        vec![
            ("nonsense", r#"{"command":"x"}"#),
            ("run", r#"{"command":"uptime","why":"load"}"#),
        ],
        vec![],
    ]);
    session(host.clone(), script).send("go").await;

    assert!(host.commands().is_empty(), "ran: {:?}", host.commands());
}

/// The bug from the 2026-08-17 log, in one test.
///
/// A model with tools ran five probes and then wrote its report as ordinary
/// prose -- which is how a model says it has finished. The loop read that as a
/// step that had lost its action, drew the report AND told the model nothing had
/// happened, and the model dutifully sent `done`: a whole extra request, and a
/// second card under an answer the user had already read.
#[tokio::test]
async fn prose_after_acting_ends_the_task() {
    for finish in [
        Reply::text("Rocky Linux 9.6, 16 cores."),
        Reply::calls(Vec::new()),
    ] {
        let host = Fake::new(permissive());
        let script = Script::scripted(vec![
            Reply::calls(vec![ToolCall {
                id: "c1".into(),
                name: "run".into(),
                arguments: r#"{"command":"uname -a","why":"identify"}"#.into(),
            }]),
            finish,
        ]);
        let asked = script.clone();
        let session = session(host.clone(), script);
        session.send("report the config").await;

        // Two requests: the probe, and the answer. Not a third.
        assert_eq!(asked.asked.load(Ordering::SeqCst), 2);
        assert!(
            host.tags().contains(&"reply".to_string()),
            "tags: {:?}",
            host.tags()
        );
        assert!(
            !session
                .history()
                .into_iter()
                .any(|message| message.content.contains("NOTHING")),
            "the model was told its finished task was still waiting",
        );
    }
}

/// Before anything has run, prose is an answer -- a question that wanted a
/// sentence gets one, and nothing is waiting on an action.
#[tokio::test]
async fn prose_before_anything_runs_is_never_nudged() {
    let host = Fake::new(permissive());
    let script = Script::scripted(vec![Reply::text("hello there")]);
    let asked = script.clone();
    session(host.clone(), script).send("hi").await;

    assert_eq!(asked.asked.load(Ordering::SeqCst), 1);
    assert!(host.commands().is_empty());
}

/// The commands a step actually asked for have to reach the log.
///
/// On the tool track the model's prose is routinely empty -- the commands are in
/// `tool_calls` -- so a log that recorded only the prose recorded a model that
/// said nothing and then, somehow, ran five things. The transcript is rendered
/// the same way the conversation itself is, so the log and the next request
/// cannot disagree about what was asked for.
#[test]
fn the_response_log_carries_the_calls_and_not_only_the_prose() {
    let reply = Reply {
        text: "Checking the disks.".into(),
        calls: vec![
            ToolCall {
                id: "c1".into(),
                name: "run".into(),
                arguments: r#"{"command":"df -h","why":"disk"}"#.into(),
            },
            ToolCall {
                id: "c2".into(),
                name: "run".into(),
                arguments: r#"{"command":"free -m","why":"memory"}"#.into(),
            },
        ],
    };

    let written = crate::ai::session::transcribe_for_test(&reply);
    assert!(
        written.contains("Checking the disks."),
        "the prose is kept: {written}"
    );
    assert!(
        written.contains("\"command\":\"df -h\""),
        "the first call is there: {written}"
    );
    assert!(
        written.contains("\"command\":\"free -m\""),
        "the second call is there: {written}"
    );
    assert!(
        written.contains("\"action\":\"run\""),
        "the verb comes back as the action"
    );
}
