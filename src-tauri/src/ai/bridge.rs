//! What connects the loop to this window: the real host, and the commands the
//! chat panel calls.
//!
//! `session.rs` knows nothing about SSH, Tauri or the config file. Everything it
//! needs arrives through the six traits in `host.rs`, and this is where those are
//! implemented against the things that actually exist -- the terminal the panel
//! borrows, the five stores on disk, and the channel back to the page.
//!
//! The confirmation round trip is the only unusual shape here. The loop awaits a
//! `bool`; the answer comes back through a separate command minutes later. So a
//! question is posted with an id, a [`oneshot`] sender is left under that id, and
//! `ai_answer` completes it. A panel that closes with a question outstanding drops
//! the senders, which resolves them as "declined" -- the safe answer.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use encoding_rs::Encoding;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::ipc::Channel;
use tokio::sync::oneshot;

use crate::config::{self, AppConfig, Language};
use crate::i18n::t;
use crate::secrets::Store as Secrets;
use crate::ssh;

use super::cancel::Cancel;
use super::context::{build_context, ContextInput, MachineFacts};
use super::files::{self, FileOpError, FilePlan, FileRequest, FileRunner};
use super::host::{AgentConfig, Asker, Bytes, Emitter, Executor, Judge, Stores};
use super::llm::http::{HttpProvider, Unconfigured};
use super::llm::retry::WithRetry;
use super::llm::LlmProvider;
use super::moves::{
    self, PathKind, TransferContext, TransferOpError, TransferPlan, TransferRequest,
};
use super::policy::command::{classify_command, PolicyResult};
use super::policy::path::{classify_path, PathPolicyResult};
use super::policy::risk::RiskReason;
use super::prompt::{build_system_prompt, MemoryPrompt, PromptInput};
use super::session::AgentSession;
use super::settings::{model_secret_key, AiSettings};
use super::shell::{AgentShell, RunOptions, TerminalIo, DEFAULT_OUTPUT_BUDGET};
use super::store::chat::{now_ms, ChatEntry, ChatRecord, ChatStore, ChatUsage};
use super::store::log::LogStore;
use super::store::memory::{AppendResult, MemoryStore, RemoveOutcome};
use super::store::skill::{SkillRead, SkillStore};
use super::store::trust::TrustStore;
use super::types::{AgentEvent, AgentMode, CommandResult, MemoryScope, TransferKind};

// ------------------------------------------------------------- where things live ---

fn ai_dir(name: &str) -> PathBuf {
    config::data_dir().join(name)
}

/// The five stores, opened once and shared by every panel.
///
/// One set for the whole window rather than one per panel: memory and skills are
/// the same files whichever conversation is reading them, and two panels holding
/// separate undo maps over one file would each be able to take back the other's
/// line.
pub struct AiStores {
    pub memory: MemoryStore,
    pub skills: SkillStore,
    pub trust: TrustStore,
    pub chats: ChatStore,
    pub logs: LogStore,
}

impl Default for AiStores {
    fn default() -> Self {
        Self {
            memory: MemoryStore::new(ai_dir("memory")),
            skills: SkillStore::new(ai_dir("skills")),
            trust: TrustStore::new(ai_dir("trusted.json")),
            chats: ChatStore::new(ai_dir("chats")),
            logs: LogStore::new(ai_dir("logs")),
        }
    }
}

// ------------------------------------------------------------------- the host ---

/// Types into the terminal the panel borrows.
struct Typist {
    sessions: Arc<ssh::Sessions>,
    terminal: String,
    encoding: &'static Encoding,
}

impl TerminalIo for Typist {
    fn write(&self, text: &str) {
        self.sessions.input(&self.terminal, self.encoding, text);
    }
}

pub struct Host {
    /// The chat panel's own pane id, which names its channel and its record.
    pane: String,
    /// The terminal pane whose shell this borrows. Its commands are typed there.
    terminal: String,
    server_id: String,
    server_name: String,
    language: Language,

    sessions: Arc<ssh::Sessions>,
    transfers: Arc<crate::transfer::Transfers>,
    target: ssh::Target,
    agent: Arc<AgentShell>,
    typist: Typist,
    stores: Arc<AiStores>,
    settings: Mutex<AiSettings>,

    out: Channel<Value>,
    record: Mutex<ChatRecord>,

    /// Questions waiting on the user, by id.
    pending: Mutex<HashMap<u64, oneshot::Sender<bool>>>,
    next_id: AtomicU64,
}

impl Host {
    fn post(&self, message: Value) {
        let _ = self.out.send(message);
    }

    fn settings(&self) -> AiSettings {
        self.settings.lock().unwrap().clone()
    }

    /// A question for the user, and the answer when it comes back.
    async fn ask(&self, message: Value) -> bool {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let mut message = message;
        message["id"] = json!(id);
        self.post(message);

        // A dropped sender -- the panel closed, or the window went -- resolves as
        // declined, which is the answer that changes nothing.
        rx.await.unwrap_or(false)
    }

    pub fn answer(&self, id: u64, ok: bool) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(ok);
        }
    }

    /// Runs a command through the borrowed terminal.
    async fn run(&self, command: &str, options: RunOptions, cancel: &Cancel) -> CommandResult {
        let timeout = self.settings().agent.command_timeout_ms;
        if !self.sessions.is_live(&self.terminal) {
            return CommandResult {
                output: "The terminal this assistant was opened from is no longer connected, so \
                         nothing could be run. Reconnect it, or open the assistant from a live \
                         terminal."
                    .into(),
                exit_code: 1,
                timed_out: false,
                truncated: false,
            };
        }
        match self.agent.run(&self.typist, command, timeout, cancel, &options).await {
            Ok(result) => result,
            Err(message) => {
                CommandResult { output: message, exit_code: 1, timed_out: false, truncated: false }
            }
        }
    }

    /// What `pwd` says, asked rather than replayed. See `context.rs`.
    async fn working_directory(&self, cancel: &Cancel) -> String {
        let silent = RunOptions { silent: true, ..Default::default() };
        let result = self.run("pwd", silent, cancel).await;
        if result.exit_code == 0 {
            result.output.trim().to_string()
        } else {
            String::new()
        }
    }

    /// The machine's own answers, asked once per task.
    async fn machine_facts(&self, cancel: &Cancel) -> MachineFacts {
        let silent = RunOptions { silent: true, ..Default::default() };
        let result = self
            .run(
                "uname -o 2>/dev/null || uname -s; uname -r; echo \"$SHELL\"; id -un; echo \"$HOME\"",
                silent,
                cancel,
            )
            .await;
        let mut lines = result.output.lines();
        MachineFacts {
            os: lines.next().unwrap_or_default().trim().to_string(),
            kernel: lines.next().unwrap_or_default().trim().to_string(),
            shell: lines.next().unwrap_or_default().trim().to_string(),
            user: lines.next().unwrap_or_default().trim().to_string(),
            home: lines.next().unwrap_or_default().trim().to_string(),
        }
    }

    /// The system prompt as it stands right now.
    ///
    /// Rebuilt per task rather than kept, because memory, the skill manifest and
    /// the thinking switch all change it between messages.
    async fn system_prompt(&self, identity: &str, cancel: &Cancel) -> String {
        let settings = self.settings();
        let facts = self.machine_facts(cancel).await;
        let cwd = self.working_directory(cancel).await;

        let context = build_context(&ContextInput {
            host: &self.server_name,
            machine: &facts,
            transcript: &self.agent.transcript(),
            cwd: &cwd,
            send_output: settings.send_terminal_output,
            output_lines: settings.output_lines,
            budget: 0,
        });

        let memory = settings.memory.enabled.then(|| MemoryPrompt {
            global: self.stores.memory.read(MemoryScope::Global, None),
            server: self.stores.memory.read(MemoryScope::Server, Some(&self.server_id)),
            server_name: self.server_name.clone(),
        });
        let skills = if settings.skills.enabled {
            self.stores.skills.manifest(&settings.skills.disabled)
        } else {
            Vec::new()
        };
        let places = moves::describe_local_places("");

        build_system_prompt(&PromptInput {
            language: self.language,
            context: &context,
            identity,
            memory: memory.as_ref(),
            local_places: &places,
            skills: &skills,
            thinking: settings.thinking.enabled,
        })
    }

    /// The two sides of a transfer, bound to which way it is going.
    fn sides(&self, kind: TransferKind) -> Sides<'_> {
        Sides { host: self, kind }
    }

    async fn probe_remote(&self, path: &str) -> Option<PathKind> {
        let silent = RunOptions { silent: true, ..Default::default() };
        let result = self.run(&files::build_probe(path), silent, &Cancel::new()).await;
        match result.output.trim().lines().next_back().unwrap_or_default().trim() {
            "dir" => Some(PathKind::Directory),
            "file" => Some(PathKind::File),
            _ => None,
        }
    }

    fn probe_local(path: &str) -> Option<PathKind> {
        let meta = std::fs::metadata(path).ok()?;
        Some(if meta.is_dir() { PathKind::Directory } else { PathKind::File })
    }
}

// --------------------------------------------------------------- the six traits ---

impl Judge for Host {
    fn classify(&self, command: &str) -> PolicyResult {
        classify_command(command, &self.settings().agent.read_only_commands)
    }
    fn classify_path(&self, path: &str) -> PathPolicyResult {
        classify_path(path)
    }
    fn describe_risk(&self, reasons: &[RiskReason]) -> Vec<String> {
        reasons.iter().map(|reason| t(self.language, &super::types::risk_key(*reason))).collect()
    }
}

impl Executor for Host {
    async fn execute(&self, command: &str, cancel: &Cancel) -> CommandResult {
        self.run(command, RunOptions::default(), cancel).await
    }
}

impl Asker for Host {
    fn mode(&self) -> AgentMode {
        self.settings().agent.mode
    }

    async fn confirm_command(&self, command: &str, why: &str, policy: &PolicyResult) -> bool {
        self.ask(json!({
            "type": "confirm",
            "command": command,
            "why": why,
            "blocker": policy.blocker.clone().unwrap_or_default(),
        }))
        .await
    }

    async fn confirm_file(&self, plan: &FilePlan, why: &str) -> bool {
        // `trustDir` rides along so the dialog's third button has something to
        // name. The page sends back only the answer, so the shell remembers the
        // directory from here -- see `fromChat`.
        self.ask(json!({
            "type": "confirmFile",
            "kind": plan.kind.tag(),
            "path": plan.path,
            "trustDir": files::dir_of(&plan.path),
            "exists": plan.exists,
            "size": files::format_bytes(plan.bytes),
            "preview": plan.preview,
            "before": plan.before.clone().unwrap_or_default(),
            "after": plan.after.clone().unwrap_or_default(),
            "encoding": plan.encoding,
            "line": plan.line,
            "why": why,
        }))
        .await
    }
}

impl FileRunner for Host {
    fn run<'a>(
        &'a self,
        command: String,
        options: RunOptions,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CommandResult> + Send + 'a>> {
        Box::pin(async move { self.run(&command, options, &Cancel::new()).await })
    }
}

impl Bytes for Host {
    async fn plan_file(
        &self,
        request: FileRequest,
        _cancel: &Cancel,
    ) -> Result<FilePlan, FileOpError> {
        files::plan_file_op(&request, self).await
    }

    async fn apply_file(&self, plan: &FilePlan, _cancel: &Cancel) -> CommandResult {
        files::apply_file_op(plan, self).await
    }

    async fn plan_transfer(
        &self,
        request: TransferRequest,
        _cancel: &Cancel,
    ) -> Result<TransferPlan, TransferOpError> {
        let sides = self.sides(request.kind);
        moves::plan_transfer(&request, &sides).await
    }

    async fn run_transfer(&self, plan: &TransferPlan, cancel: &Cancel) -> CommandResult {
        self.move_bytes(plan, cancel).await
    }
}

impl Stores for Host {
    fn memory_enabled(&self) -> bool {
        self.settings().memory.enabled
    }

    async fn remember(&self, scope: MemoryScope, text: &str) -> AppendResult {
        let settings = self.settings();
        let budget = match scope {
            MemoryScope::Global => settings.memory.global_budget,
            MemoryScope::Server => settings.memory.server_budget,
        };
        self.stores.memory.append(scope, self.scope_id(scope), text, budget)
    }

    async fn forget(&self, scope: MemoryScope, text: &str) -> RemoveOutcome {
        self.stores.memory.remove(scope, self.scope_id(scope), text)
    }

    fn skills_enabled(&self) -> bool {
        self.settings().skills.enabled
    }

    async fn load_skill(&self, id: &str, file: Option<&str>) -> SkillRead {
        let settings = self.settings();
        self.stores.skills.read(id, file, settings.skills.max_chars, &settings.skills.disabled)
    }

    async fn is_trusted(&self, path: &str) -> bool {
        self.stores.trust.is_trusted(&self.server_id, path)
    }
}

impl Emitter for Host {
    fn emit(&self, event: AgentEvent) {
        /*
         * Recorded before it is posted, and not all of it.
         *
         * The transient events describe a moment rather than the conversation --
         * a progress bar frozen at 43% replayed a week later would be reporting
         * something that is no longer true of anything. The rest is the thread,
         * and it is what a reopened conversation is drawn from.
         */
        let keep = !matches!(
            event,
            AgentEvent::Thinking { .. }
                | AgentEvent::Delta { .. }
                | AgentEvent::Reasoning { .. }
                | AgentEvent::TransferProgress { .. }
                | AgentEvent::Context { .. }
                | AgentEvent::Transport { .. }
                | AgentEvent::Idle
        );
        if keep {
            let mut record = self.record.lock().unwrap();
            record.entries.push(ChatEntry::event(event.clone()));
            if let AgentEvent::Usage { prompt, completion, estimated } = &event {
                let usage = record.usage.get_or_insert(ChatUsage::default());
                usage.prompt += *prompt as u64;
                usage.completion += *completion as u64;
                usage.requests += 1;
                // Never cleared: a total that is half measured and half guessed
                // is a guess, and saying otherwise would be dishonest.
                usage.estimated = usage.estimated || estimated.unwrap_or(false);
            }
            let mut copy = record.clone();
            drop(record);
            self.stores.chats.save(&mut copy);
            *self.record.lock().unwrap() = copy;
        }
        self.post(json!({ "type": "event", "event": event }));
    }
}

impl Host {
    fn scope_id(&self, scope: MemoryScope) -> Option<&str> {
        match scope {
            MemoryScope::Global => None,
            MemoryScope::Server => Some(&self.server_id),
        }
    }

    /// Moves the bytes, auto-skipping anything already at the destination.
    ///
    /// The engine is the transfer page's, unchanged. What differs is who answers
    /// its conflict prompts: nobody is here to, and overwriting on the
    /// assistant's own initiative is the one outcome that cannot be taken back --
    /// so every prompt is answered "skip" the moment it arrives, and what was
    /// skipped is reported to the model instead.
    async fn move_bytes(&self, plan: &TransferPlan, cancel: &Cancel) -> CommandResult {
        let (from, to) = match plan.kind {
            TransferKind::Upload => ("local", "remote"),
            TransferKind::Download => ("remote", "local"),
        };
        let target = match self.dial() {
            Ok(target) => target,
            Err(message) => {
                return CommandResult {
                    output: format!("The transfer could not start: {message}"),
                    exit_code: 1,
                    timed_out: false,
                    truncated: false,
                }
            }
        };

        // Its own pane id, so a transfer the assistant starts cannot be cancelled
        // by the transfer tab's stop button, or the other way round.
        let pane = format!("ai:{}", self.pane);
        let skipped: Arc<Mutex<Vec<String>>> = Arc::default();
        let failures: Arc<Mutex<Vec<String>>> = Arc::default();

        let transfers = self.transfers.clone();
        let out = self.out.clone();
        let watching = pane.clone();
        let seen_skips = skipped.clone();
        let seen_failures = failures.clone();
        let channel = Channel::new(move |message: tauri::ipc::InvokeResponseBody| {
            let Ok(text) = message.deserialize::<Value>() else { return Ok(()) };
            match text["kind"].as_str().unwrap_or_default() {
                "conflict" => {
                    if let Some(path) = text["targetPath"].as_str() {
                        seen_skips.lock().unwrap().push(path.to_string());
                    }
                    if let Some(id) = text["id"].as_u64() {
                        transfers.answer(&watching, id, "skip");
                    }
                }
                "progress" => {
                    let _ = out.send(json!({
                        "type": "event",
                        "event": {
                            "type": "transferProgress",
                            "progress": {
                                "done": text["done"],
                                "total": text["total"],
                                "filesDone": text["filesDone"],
                                "filesTotal": text["filesTotal"],
                                "name": text["name"],
                            }
                        }
                    }));
                }
                "item" if text["status"] == "failed" => {
                    seen_failures
                        .lock()
                        .unwrap()
                        .push(text["path"].as_str().unwrap_or_default().to_string());
                }
                _ => {}
            }
            Ok(())
        });

        // The stop button reaches the engine through its own cancel flag.
        let stopping = self.transfers.clone();
        let stop_pane = pane.clone();
        let watcher = cancel.clone();
        tauri::async_runtime::spawn(async move {
            watcher.cancelled().await;
            stopping.cancel(&stop_pane);
        });

        let roots: Vec<(String, bool)> =
            plan.roots.iter().map(|root| (root.path.clone(), root.is_directory)).collect();
        let outcome = crate::transfer::run(
            self.transfers.clone(),
            pane,
            from.to_string(),
            to.to_string(),
            target,
            roots,
            plan.target.clone(),
            channel,
        )
        .await;

        match outcome {
            Ok(summary) => moves::describe_transfer(
                plan,
                &moves::TransferSummary {
                    completed: summary.completed,
                    skipped: summary.skipped,
                    failed: summary.failed,
                    cancelled: summary.cancelled,
                },
                &skipped.lock().unwrap(),
                &failures.lock().unwrap(),
            ),
            Err(message) => CommandResult {
                output: format!("The {} failed before it finished: {message}", match plan.kind {
                    TransferKind::Upload => "upload",
                    TransferKind::Download => "download",
                }),
                exit_code: 1,
                timed_out: false,
                truncated: false,
            },
        }
    }

    /// The connection details for this panel's server.
    ///
    /// Resolved once, when the panel opened, and reused: the credentials are the
    /// terminal's own, so a transfer never asks for a password the user has
    /// already given.
    fn dial(&self) -> Result<ssh::Target, String> {
        Ok(self.target.clone())
    }
}

/// The two sides of one transfer, bound to its direction.
struct Sides<'a> {
    host: &'a Host,
    kind: TransferKind,
}

impl Sides<'_> {
    fn source_is_local(&self) -> bool {
        self.kind == TransferKind::Upload
    }
}

impl TransferContext for Sides<'_> {
    async fn probe_source(&self, path: &str) -> Option<PathKind> {
        if self.source_is_local() {
            Host::probe_local(path)
        } else {
            self.host.probe_remote(path).await
        }
    }
    async fn probe_target(&self, path: &str) -> Option<PathKind> {
        if self.source_is_local() {
            self.host.probe_remote(path).await
        } else {
            Host::probe_local(path)
        }
    }
    fn normalize_source(&self, path: &str) -> String {
        path.trim().to_string()
    }
    fn normalize_target(&self, path: &str) -> String {
        path.trim().to_string()
    }
    fn home_source(&self) -> String {
        if self.source_is_local() {
            dirs::home_dir().map(|home| home.display().to_string()).unwrap_or_default()
        } else {
            String::new()
        }
    }
    fn home_target(&self) -> String {
        if self.source_is_local() {
            String::new()
        } else {
            dirs::home_dir().map(|home| home.display().to_string()).unwrap_or_default()
        }
    }
    async fn default_target(&self) -> String {
        // Only an upload has one: the directory the user is standing in.
        if self.source_is_local() {
            self.host.working_directory(&Cancel::new()).await
        } else {
            String::new()
        }
    }
    async fn pick_sources(&self) -> Vec<String> {
        // A picker needs a window, and one is not wired yet: the model is told
        // to name the path instead, which is what the empty answer means.
        Vec::new()
    }
    async fn pick_target(&self) -> String {
        String::new()
    }
}

// -------------------------------------------------------------------- panels ---

pub struct Panel {
    pub host: Arc<Host>,
    pub session: AgentSession<Host>,
}

#[derive(Default)]
pub struct Panels {
    live: Mutex<HashMap<String, Arc<Panel>>>,
}

impl Panels {
    pub fn get(&self, pane: &str) -> Option<Arc<Panel>> {
        self.live.lock().unwrap().get(pane).cloned()
    }

    pub fn insert(&self, pane: &str, panel: Arc<Panel>) {
        self.live.lock().unwrap().insert(pane.to_string(), panel);
    }

    pub fn remove(&self, pane: &str) -> Option<Arc<Panel>> {
        self.live.lock().unwrap().remove(pane)
    }

    /// Every panel open in this window, for a setting that changed under all of
    /// them.
    pub fn all(&self) -> Vec<Arc<Panel>> {
        self.live.lock().unwrap().values().cloned().collect()
    }
}

/// The provider the settings currently point at, wrapped in its retry.
fn provider_for(settings: &AiSettings, secrets: &Secrets) -> Arc<dyn LlmProvider> {
    match settings.active_model() {
        None => Arc::new(Unconfigured),
        Some(model) => {
            let key = secrets.get(&model_secret_key(&model.id)).unwrap_or_default();
            Arc::new(WithRetry::new(HttpProvider::new(
                &model.base_url,
                &model.model,
                &key,
                settings.thinking.clone(),
            )))
        }
    }
}

/// What the panel is handed when it opens.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelBootstrap {
    pub language: &'static str,
    pub host: String,
    pub mode: &'static str,
    pub model: String,
    pub thinking: Value,
    pub enabled: bool,
    /// The conversation being resumed, when one is.
    pub entries: Vec<ChatEntry>,
    pub usage: Option<ChatUsage>,
}

fn fresh_record(server_id: &str, server_name: &str) -> ChatRecord {
    ChatRecord {
        id: config::make_id(),
        server_id: server_id.to_string(),
        server_name: server_name.to_string(),
        title: String::new(),
        created_at: now_ms(),
        updated_at: now_ms(),
        entries: Vec::new(),
        messages: Vec::new(),
        usage: None,
    }
}

fn thinking_view(settings: &AiSettings) -> Value {
    json!({
        "enabled": settings.thinking.enabled,
        "effort": settings.thinking.effort.tag(),
        "show": settings.thinking.show,
    })
}

fn model_label(settings: &AiSettings, language: Language) -> String {
    match settings.active_model() {
        Some(model) => model.model.clone(),
        None => t(language, "agentModelWindowTitle"),
    }
}

/// Opens a chat panel over one terminal.
#[allow(clippy::too_many_arguments)]
pub fn open_panel(
    pane: String,
    terminal: String,
    server_id: String,
    server_name: String,
    encoding: &'static Encoding,
    target: ssh::Target,
    sessions: Arc<ssh::Sessions>,
    transfers: Arc<crate::transfer::Transfers>,
    stores: Arc<AiStores>,
    secrets: &Secrets,
    config: &AppConfig,
    out: Channel<Value>,
) -> (Arc<Panel>, PanelBootstrap) {
    let settings = config.settings.ai.clone();
    let language = config.settings.language;

    let agent = Arc::new(AgentShell::fresh(settings.agent.output_budget.max(DEFAULT_OUTPUT_BUDGET)));
    // From here every byte of that terminal passes through the agent on its way
    // to the tab, which is what lets a command's own output be told apart.
    sessions.attach(&terminal, agent.clone());

    let log = Arc::new(stores.logs.open(settings.log.enabled, settings.log.keep, &server_name));

    let host = Arc::new(Host {
        pane: pane.clone(),
        terminal: terminal.clone(),
        server_id: server_id.clone(),
        server_name: server_name.clone(),
        language,
        sessions: sessions.clone(),
        transfers,
        target,
        agent,
        typist: Typist { sessions, terminal, encoding },
        stores,
        settings: Mutex::new(settings.clone()),
        out,
        record: Mutex::new(fresh_record(&server_id, &server_name)),
        pending: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
    });

    let mut agent_config = AgentConfig::new(provider_for(&settings, secrets));
    agent_config.max_steps = settings.agent.max_steps;
    agent_config.context_budget = settings.agent.context_budget;
    agent_config.request_timeout_ms = settings.request_timeout_ms;

    let session = AgentSession::new(host.clone(), agent_config).with_log(log);
    let panel = Arc::new(Panel { host: host.clone(), session });

    let bootstrap = PanelBootstrap {
        language: language.tag(),
        host: server_name,
        mode: settings.agent.mode.tag(),
        model: model_label(&settings, language),
        thinking: thinking_view(&settings),
        enabled: settings.enabled,
        entries: Vec::new(),
        usage: None,
    };
    (panel, bootstrap)
}

impl Panel {
    /// Re-reads the config, so a model, mode or budget changed elsewhere takes
    /// effect on the next message rather than on the next window.
    pub fn refresh(&self, config: &AppConfig, secrets: &Secrets) {
        let settings = config.settings.ai.clone();
        *self.host.settings.lock().unwrap() = settings.clone();
        let provider = provider_for(&settings, secrets);
        self.session.configure(|current| {
            current.provider = provider;
            current.max_steps = settings.agent.max_steps;
            current.context_budget = settings.agent.context_budget;
            current.request_timeout_ms = settings.request_timeout_ms;
        });
    }

    /// Runs one message to completion, rebuilding the prompt first.
    pub async fn send(&self, text: String) {
        let identity = self.session.describe_provider();
        let cancel = Cancel::new();
        let system = self.host.system_prompt(&identity, &cancel).await;
        self.session.configure(|current| current.system = system);

        let named = {
            let mut record = self.host.record.lock().unwrap();
            let first = record.title.is_empty();
            if first {
                // Written once, from the opening message, and then left alone: a
                // tab that renamed itself on every turn would be unfindable.
                record.title = super::store::chat::title_for(&text);
            }
            record.entries.push(ChatEntry::user(text.clone()));
            first.then(|| record.title.clone())
        };
        if let Some(title) = named {
            self.host.post(json!({
                "type": "title",
                "text": super::store::chat::panel_title(
                    &title,
                    &self.host.server_name,
                    &t(self.host.language, "agentTitle"),
                ),
            }));
        }
        self.host.post(json!({ "type": "user", "text": text }));
        self.host.post(json!({ "type": "state", "running": true }));

        self.session.send(&text).await;

        // The conversation as the model saw it, kept so a reopened panel carries
        // on rather than starting again knowing nothing.
        let mut record = self.host.record.lock().unwrap().clone();
        record.messages = self.session.history();
        self.host.stores.chats.save(&mut record);
        *self.host.record.lock().unwrap() = record;

        self.host.post(json!({ "type": "state", "running": false }));
    }

    pub fn close(&self) {
        self.session.stop();
        self.host.sessions.detach(&self.host.terminal);
        // Anything still waiting on the user resolves as declined when the
        // senders drop with the map.
        self.host.pending.lock().unwrap().clear();
    }

    pub fn record_id(&self) -> String {
        self.host.record.lock().unwrap().id.clone()
    }

    pub fn server_id(&self) -> String {
        self.host.server_id.clone()
    }

    /// Starts a fresh conversation, keeping the panel and its terminal.
    pub fn new_chat(&self) {
        self.session.reset();
        *self.host.record.lock().unwrap() =
            fresh_record(&self.host.server_id, &self.host.server_name);
        self.host.post(json!({ "type": "cleared" }));
    }

    /// Puts a recorded conversation back on screen and into the model's context.
    ///
    /// Both halves, because reloading has to satisfy two readers: `entries` is
    /// what the panel drew, and `messages` is what the model was told. Restoring
    /// only the first would put the thread back and leave the next message
    /// starting a task the model knows nothing about.
    pub fn load(&self, id: &str) -> Option<Value> {
        let record = self.host.stores.chats.load(id)?;
        self.session.restore(record.messages.clone());
        let view = json!({
            "type": "restore",
            "entries": record.entries,
            "id": record.id,
            "usage": record.usage,
        });
        *self.host.record.lock().unwrap() = record;
        Some(view)
    }

    /// Draws the row that says a directory is now trusted.
    ///
    /// Reported the way a remembered line is, and for the same reason: it is a
    /// lasting change to how the assistant behaves, made in one keystroke, and
    /// the dialog that made it is gone a moment later. The row is where you find
    /// out it happened, and how to undo it.
    pub fn note_trusted(&self, dir: &str) {
        self.host.emit(AgentEvent::Trusted { dir: dir.to_string() });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stores_all_sit_under_the_data_directory() {
        let stores = AiStores::default();
        let root = config::data_dir();
        assert!(stores.memory.file_for(MemoryScope::Global, None).starts_with(&root));
        assert!(stores.skills.root().starts_with(&root));
        assert!(stores.trust.path().starts_with(&root));
        assert!(stores.logs.dir().starts_with(&root));
    }

    #[test]
    fn a_fresh_record_is_stamped_and_empty() {
        let record = fresh_record("s1", "web-1");
        assert!(!record.id.is_empty());
        assert_eq!(record.server_id, "s1");
        assert!(record.title.is_empty());
        assert!(record.entries.is_empty());
        assert!(record.usage.is_none());
        assert!(record.created_at > 0);
    }

    #[test]
    fn with_no_endpoint_configured_the_provider_says_so_rather_than_being_absent() {
        let settings = AiSettings::default();
        let secrets = Secrets::open(&std::env::temp_dir().join("tshell-bridge-test"));
        let provider = provider_for(&settings, &secrets);
        assert_eq!(provider.id(), "unconfigured");
    }

    #[test]
    fn the_toolbar_falls_back_to_a_label_when_nothing_is_configured() {
        let settings = AiSettings::default();
        assert!(!model_label(&settings, Language::EnUs).is_empty());

        let mut settings = AiSettings::default();
        settings.add_model("http://x", "gpt-x");
        assert_eq!(model_label(&settings, Language::EnUs), "gpt-x");
    }

    #[test]
    fn the_thinking_view_is_the_shape_the_page_reads() {
        let view = thinking_view(&AiSettings::default());
        assert_eq!(view["enabled"], true);
        assert_eq!(view["effort"], "high");
        assert_eq!(view["show"], false);
    }
}
