//! The one seam between the loop and whichever model answers.
//!
//! Kept free of anything to do with SSH or the window, so the loop can be
//! reasoned about on its own and a test can answer a request without a network.

pub mod http;
pub mod retry;
pub mod say;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self { role: ChatRole::User, content: content.into() }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self { role: ChatRole::Assistant, content: content.into() }
    }
}

/// What one attempt cost, when the provider was told.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt: u32,
    pub completion: u32,
}

/// How the answer actually arrived, as opposed to how it was asked for.
///
/// Every request asks for a stream, and a good many endpoints quietly do not
/// give one -- a gateway that ignores `stream`, a proxy that buffers the whole
/// body before passing it on. The panel cannot tell the difference by itself: an
/// answer handed over in one piece and an answer written very fast look identical
/// once they are on screen, so "is it still streaming?" is a question the user
/// has no way to answer and no reason not to ask.
///
/// `frames` is what settles it where `streamed` cannot. A buffering proxy returns
/// a genuine event stream carrying the entire answer in one or two frames, which
/// is streaming by content-type and not by any measure the reader cares about.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportInfo {
    /// True when the body was server-sent events rather than one JSON document.
    pub streamed: bool,
    /// Content-bearing frames. One means the answer arrived whole, however sent.
    pub frames: u32,
    /// Milliseconds from the request to the first content, and to the last.
    pub first_ms: u64,
    pub total_ms: u64,
}

/// A request field that not every "OpenAI-compatible" endpoint understands.
///
/// They are named rather than lumped together because the user is told which one
/// went missing, and the two mean different things: without `thinking` the model
/// thinks whether or not it was asked to, and without `reasoning_effort` it
/// thinks as hard as it likes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExtraField {
    Thinking,
    ReasoningEffort,
}

impl ExtraField {
    pub fn key(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::ReasoningEffort => "reasoning_effort",
        }
    }
}

/// Recognisable failures, so the panel can offer the right next step.
///
/// `Empty` is the odd one out: a well-formed 200 that carried no answer. It is a
/// hiccup rather than a refusal, which is why it is worth telling apart -- the
/// loop retries it once, where it gives up on everything else here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmFailure {
    NotConfigured,
    Unauthorized,
    RateLimited,
    Network,
    Timeout,
    Cancelled,
    Empty,
    Unknown,
}

impl LlmFailure {
    /// The i18n key the panel shows for this failure.
    pub fn key(self) -> &'static str {
        match self {
            Self::NotConfigured => "aiNotConfigured",
            Self::Unauthorized => "aiUnauthorized",
            Self::RateLimited => "aiRateLimited",
            Self::Network => "aiNetwork",
            Self::Timeout => "aiTimeout",
            Self::Cancelled => "aiCancelled",
            Self::Empty => "aiEmpty",
            Self::Unknown => "aiFailed",
        }
    }

    /// The bare word, for the `retry` event's `kind`.
    pub fn tag(self) -> &'static str {
        match self {
            Self::NotConfigured => "notConfigured",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited => "rateLimited",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Empty => "empty",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    pub kind: LlmFailure,
    pub message: String,
}

impl LlmError {
    pub fn new(kind: LlmFailure, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.message)
    }
}

impl std::error::Error for LlmError {}

/// Everything a request wants to be told about while it is in flight.
///
/// One trait rather than a struct of callbacks, because every one of these is
/// answered by the same object -- the session -- and passing six closures that
/// all capture it would mean six `Arc`s where one does.
pub trait Watcher: Send + Sync {
    /// Prose meant for the user, as it arrives. Called only for the part of the
    /// reply that is an answer -- never for the JSON scaffolding around it.
    fn delta(&self, _text: &str) {}
    /// The model's own thinking, as it arrives, for the models that expose it.
    fn reasoning(&self, _text: &str) {}
    /// Called once per attempt that reported token counts. Never for one that did not.
    fn usage(&self, _usage: TokenUsage) {}
    /// Called once per attempt that produced an answer, describing how it arrived.
    fn transport(&self, _info: TransportInfo) {}
    /// Called before a failed attempt is tried again, so the wait can be explained.
    fn retry(&self, _attempt: u32, _kind: LlmFailure) {}
    /// The endpoint refused one of the optional fields, so it was dropped and the
    /// request was sent again without it.
    ///
    /// Worth saying out loud rather than swallowing: the user asked for something
    /// -- no thinking, less thinking -- and did not get it. The reply that follows
    /// is a real reply, which is why this is not an error, and it was produced
    /// under settings other than the ones on screen, which is why it cannot be
    /// silent.
    fn degraded(&self, _fields: &[ExtraField]) {}
    /// Whether text found in the thinking field may stand in for an empty answer.
    ///
    /// A reasoning model sometimes hands back an empty `content` with something
    /// in `reasoning_content`, and there are two entirely different reasons for
    /// it. The answer may have been written into the wrong field, in which case
    /// taking it is a rescue. Or the model simply thought and never answered, in
    /// which case the text is a plan in the first person -- and handing that back
    /// as a reply produces the same paragraph twice on screen, once as thinking
    /// and once as an answer, and ends a task that had not started.
    ///
    /// Nothing at this layer can tell the two apart, because the difference is
    /// whether the text means anything to the caller. So the caller is asked. The
    /// default is no: an empty reply is a failure the caller already knows how to
    /// retry, and a wrong rescue is worse than a clean one.
    fn salvage_reasoning(&self, _text: &str) -> bool {
        false
    }
    /// The model stopped because it ran out of room, with an answer already part
    /// written. What came back is real and worth keeping -- it is simply not the
    /// whole of what was being said, and the caller must not read it as finished.
    fn truncated(&self) {}
    /// The log, when the user has asked for one. Written whole, once per attempt.
    fn logged_request(&self, _body: &serde_json::Value) {}
    fn logged_reasoning(&self, _text: &str) {}
    fn logged_response(&self, _text: &str) {}
}

/// A watcher that wants none of it. What a test uses when it only wants the text.
pub struct Silent;
impl Watcher for Silent {}

pub struct CompletionRequest {
    pub system: String,
    /// The whole exchange so far. An agent step is only as good as what it can see.
    pub messages: Vec<ChatMessage>,
    /// How long the answer may go **silent** before it is abandoned. Not how long
    /// it may take: a model working through a large context can think for minutes,
    /// and a clock started at the request would call that a failure and throw away
    /// work that was going perfectly well. What cannot be waited on forever is
    /// silence, so the clock is reset by every byte that arrives. 0 leaves it to
    /// the transport.
    pub timeout_ms: u64,
    pub cancel: super::cancel::Cancel,
    pub watcher: Arc<dyn Watcher>,
}

pub type Completion<'a> = Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send + 'a>>;

pub trait LlmProvider: Send + Sync {
    /// Shown in error messages so the user knows which path failed.
    fn id(&self) -> &str;
    /// How the model should answer "what are you?". A model has no way of knowing
    /// which endpoint it is being served from, so it is told rather than left to
    /// guess and hedge.
    fn describe(&self) -> String;
    /// Boxed by hand rather than written `async fn`, because this is held as
    /// `Box<dyn LlmProvider>` -- the endpoint can be switched mid-session, and a
    /// generic parameter would freeze it at the type the session started with.
    fn complete<'a>(&'a self, request: CompletionRequest) -> Completion<'a>;
}
