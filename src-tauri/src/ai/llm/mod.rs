//! The one seam between the loop and whichever model answers.
//!
//! Kept free of anything to do with SSH or the window, so the loop can be
//! reasoned about on its own and a test can answer a request without a network.

pub mod http;
pub mod prose;
pub mod retry;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImage {
    /// The image's real format, verified from its decoded bytes before it enters
    /// the conversation. DeepSeek explicitly ignores filename extensions here.
    pub media_type: String,
    /// Raw base64, without a data-URL prefix. Keeping the two parts separate
    /// makes the durable history small enough to inspect and hard to mis-project.
    pub data: String,
}

impl ChatImage {
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.media_type, self.data)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    /// Vision inputs belong only to user turns. Old records omit the field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ChatImage>,
    /// Native calls made by an assistant turn. Empty for ordinary prose and for
    /// old records written before structured tool history was introduced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// The call answered by a `tool` message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
    pub fn user_with_images(content: impl Into<String>, images: Vec<ChatImage>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
            images,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }
    pub fn assistant_calls(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: ChatRole::Assistant,
            content: content.into(),
            images: Vec::new(),
            tool_calls,
            tool_call_id: None,
        }
    }
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::Tool,
            content: content.into(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: Some(tool_call_id.into()),
        }
    }
    pub fn char_len(&self) -> usize {
        self.content.chars().count()
            + self
                .tool_calls
                .iter()
                .map(|call| call.name.chars().count() + call.arguments.chars().count())
                .sum::<usize>()
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
/// went missing, and they mean different things: without `thinking` the model
/// thinks whether or not it was asked to, without `reasoning_effort` it thinks as
/// hard as it likes, and without `tools` it falls back to writing the protocol
/// out as JSON in its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExtraField {
    Thinking,
    ReasoningEffort,
    Tools,
}

impl ExtraField {
    pub fn key(self) -> &'static str {
        match self {
            Self::Thinking => "thinking",
            Self::ReasoningEffort => "reasoning_effort",
            Self::Tools => "tools",
        }
    }
}

/// One thing the model may call, described the way the endpoint wants it.
///
/// Sent on every request that has any. An endpoint that does not know the field
/// refuses the whole request, which is what [`ExtraField::Tools`] is for: it
/// comes off, the request goes again without it, and the model answers in the
/// JSON protocol the system prompt describes instead. Both are supported for the
/// life of the product -- see `ai/tools.rs`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// A JSON Schema object for the arguments.
    pub parameters: serde_json::Value,
}

/// One call the model made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The endpoint's own id for it. Carried because a model that sees its own
    /// call echoed back reads the id as part of it; nothing here matches on it.
    pub id: String,
    pub name: String,
    /// Still a string, exactly as the model wrote it.
    ///
    /// Not parsed here on purpose. Constrained decoding makes malformed arguments
    /// rare rather than impossible, and the repair this client already has for a
    /// mangled object -- see `parse.rs` -- is the same repair this needs. So it
    /// travels as text and is read in one place.
    pub arguments: String,
}

/// What one request came back with.
///
/// Both halves can be present at once and routinely are: a model that is about to
/// call a tool usually says what it is about to do first, and that sentence is
/// the user's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

impl From<&str> for Reply {
    fn from(text: &str) -> Self {
        Reply::text(text)
    }
}

impl Reply {
    /// Prose and nothing else, which on either track is the answer and the end of
    /// the task.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            calls: Vec::new(),
        }
    }

    /// A reply that carried calls and nothing else, which is the ordinary shape
    /// of a working step on the tool track.
    #[cfg(test)]
    pub fn calls(calls: Vec<ToolCall>) -> Self {
        Self {
            text: String::new(),
            calls,
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
        Self {
            kind,
            message: message.into(),
        }
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
    /// The answer, whole: the prose AND whatever it asked to call.
    ///
    /// Takes the reply rather than the text because on the tool track the text is
    /// routinely empty -- the commands are in `calls`, and a log that recorded
    /// only the prose recorded a model that said nothing and then, somehow, ran
    /// five things.
    fn logged_response(&self, _reply: &Reply) {}
}

/// A watcher that wants none of it. What a test uses when it only wants the text.
#[cfg(test)]
pub struct Silent;
#[cfg(test)]
impl Watcher for Silent {}

pub struct CompletionRequest {
    pub system: String,
    /// Used only after this endpoint has rejected native tools. Keeping it next
    /// to the native prompt makes the retry atomic: no second session turn is
    /// needed to change protocols.
    pub fallback_system: String,
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
    /// What the model may call. Empty asks for the JSON protocol instead, which
    /// is also what an endpoint that refused the field gets on the second try.
    pub tools: Vec<ToolSpec>,
}

pub type Completion<'a> = Pin<Box<dyn Future<Output = Result<Reply, LlmError>> + Send + 'a>>;

pub trait LlmProvider: Send + Sync {
    /// How the model should answer "what are you?". A model has no way of knowing
    /// which endpoint it is being served from, so it is told rather than left to
    /// guess and hedge.
    fn describe(&self) -> String;
    /// Boxed by hand rather than written `async fn`, because this is held as
    /// `Box<dyn LlmProvider>` -- the endpoint can be switched mid-session, and a
    /// generic parameter would freeze it at the type the session started with.
    fn complete<'a>(&'a self, request: CompletionRequest) -> Completion<'a>;
}
