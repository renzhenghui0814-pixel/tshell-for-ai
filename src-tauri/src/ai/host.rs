//! Everything the loop needs from the outside world, in six pieces.
//!
//! The split is by "what would a test replace", not by subject matter. The loop's
//! awkward paths -- a refused command, a declined confirmation, a timeout, a full
//! memory scope, running out of steps -- are all awkward to provoke out of a real
//! server and trivial to produce from a fake, and that is the whole reason these
//! exist.
//!
//! [`Judge`] is the odd one out: it is pure and synchronous, and a test uses the
//! real implementation because there is nothing to stub. It is a trait anyway so
//! that the loop takes its verdicts from one place rather than reaching into
//! `policy` itself, which is what lets a test force a refusal without having to
//! think up a genuinely destructive command.
//!
//! The async methods are written `-> impl Future<Output = _> + Send` rather than
//! `async fn`, because the session runs inside a tokio task and the futures have
//! to be `Send`; `async fn` in a trait cannot say so.

use std::future::Future;
use std::sync::Arc;

use super::cancel::Cancel;
use super::files::{FileOpError, FilePlan, FileRequest};
use super::llm::LlmProvider;
use super::moves::{TransferOpError, TransferPlan, TransferRequest};
use super::policy::command::PolicyResult;
use super::policy::path::PathPolicyResult;
use super::policy::risk::RiskReason;
use super::store::memory::AppendResult;
use super::store::skill::SkillRead;
use super::types::{AgentEvent, AgentMode, CommandResult, MemoryScope};

/// What may run, and where bytes may land. Pure, and the same in a test.
pub trait Judge: Send + Sync {
    fn classify(&self, command: &str) -> PolicyResult;
    fn classify_path(&self, path: &str) -> PathPolicyResult;
    /// The refusal reasons in the user's language, for the card that shows them.
    fn describe_risk(&self, reasons: &[RiskReason]) -> Vec<String>;
}

/// The only thing that types into the terminal.
pub trait Executor: Send + Sync {
    fn execute(&self, command: &str, cancel: &Cancel) -> impl Future<Output = CommandResult> + Send;
}

/// The only thing that puts a question in front of the user.
pub trait Asker: Send + Sync {
    /// How much to ask before acting.
    ///
    /// A function rather than a value, so it is read at the step that needs it.
    /// The task's other settings are fixed when the task starts, and this one must
    /// not be: someone who switches away from `auto` halfway through a long job
    /// means it for the next command, not for the next message.
    fn mode(&self) -> AgentMode;
    /// Resolves false when the user declines or dismisses the prompt.
    fn confirm_command(
        &self,
        command: &str,
        why: &str,
        policy: &PolicyResult,
    ) -> impl Future<Output = bool> + Send;
    /// Shows the user the whole change. Resolves false when they decline.
    fn confirm_file(&self, plan: &FilePlan, why: &str) -> impl Future<Output = bool> + Send;
}

/// The only thing that moves bytes.
pub trait Bytes: Send + Sync {
    /// Reads the machine and works out exactly what would change.
    fn plan_file(
        &self,
        request: FileRequest,
        cancel: &Cancel,
    ) -> impl Future<Output = Result<FilePlan, FileOpError>> + Send;
    fn apply_file(&self, plan: &FilePlan, cancel: &Cancel)
        -> impl Future<Output = CommandResult> + Send;
    /// Resolves a transfer against both machines, asking the user for anything the
    /// model could not supply. A dismissed dialog is an error here -- a transfer
    /// nobody chose a destination for has not happened.
    fn plan_transfer(
        &self,
        request: TransferRequest,
        cancel: &Cancel,
    ) -> impl Future<Output = Result<TransferPlan, TransferOpError>> + Send;
    /// Moves the bytes, reporting progress through the emitter. Never confirmed.
    fn run_transfer(
        &self,
        plan: &TransferPlan,
        cancel: &Cancel,
    ) -> impl Future<Output = CommandResult> + Send;
}

/// The only thing that touches the disk on this side.
pub trait Stores: Send + Sync {
    /// False means `remember` was never offered, so using it is a mistake.
    fn memory_enabled(&self) -> bool;
    fn remember(&self, scope: MemoryScope, text: &str) -> impl Future<Output = AppendResult> + Send;
    /// False means no skill was ever offered, so naming one is a mistake rather
    /// than a miss.
    fn skills_enabled(&self) -> bool;
    /// Reads one file out of one skill. Never fails; every failure is an outcome.
    fn load_skill(&self, id: &str, file: Option<&str>) -> impl Future<Output = SkillRead> + Send;
    /// Whether the user has already said yes to everything in this file's
    /// directory.
    ///
    /// Only consulted in `trust` mode, and only after `classify_path` has already
    /// allowed the path -- this can turn a confirmation into a silent write, never
    /// a refusal into anything at all.
    fn is_trusted(&self, path: &str) -> impl Future<Output = bool> + Send;
}

/// Where events go. A test collects them and asserts on the whole sequence.
pub trait Emitter: Send + Sync {
    fn emit(&self, event: AgentEvent);
}

pub trait AgentHost: Judge + Executor + Asker + Bytes + Stores + Emitter {}

impl<T> AgentHost for T where T: Judge + Executor + Asker + Bytes + Stores + Emitter {}

/// The parts of the wiring that change between tasks rather than between hosts.
///
/// Held as data on the session rather than asked of the host, because every one
/// of these is a setting read once when a task starts -- except the provider,
/// which can be swapped mid-session when the user picks another endpoint, and is
/// boxed for exactly that reason.
pub struct AgentConfig {
    /// An `Arc` rather than a `Box` so the loop can take a handle and let go of
    /// the lock before it awaits: the request lasts a minute, and `configure`
    /// must not be blocked behind it.
    pub provider: Arc<dyn LlmProvider>,
    pub system: String,
    pub max_steps: u32,
    /// Characters of conversation carried into one request. 0 carries everything.
    pub context_budget: usize,
    /// How long one request to the model may take. 0 leaves it to the transport.
    pub request_timeout_ms: u64,
}

impl AgentConfig {
    pub fn new(provider: Arc<dyn LlmProvider>) -> Self {
        Self {
            provider,
            system: String::new(),
            max_steps: 0,
            context_budget: 48_000,
            request_timeout_ms: 120_000,
        }
    }
}
