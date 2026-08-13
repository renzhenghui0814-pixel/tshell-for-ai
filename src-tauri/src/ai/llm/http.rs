//! Talks to any OpenAI-compatible `/chat/completions` endpoint: the official
//! API, DeepSeek, a company gateway, or a local Ollama in compatibility mode.
//!
//! Streaming is asked for and not required. `stream: true` is what makes an
//! answer appear as it is written rather than all at once after a long silence,
//! but plenty of gateways ignore it and reply with an ordinary JSON body. Both
//! are read, and which one arrived is decided by what came back rather than by
//! configuration -- there is nothing for the user to get wrong.
//!
//! The same is true of the fields beyond the ones every endpoint knows --
//! thinking, effort. They are asked for where they would change something, and an
//! endpoint that refuses one is dropped back to its default rather than failed.
//! See [`HttpProvider::extras`] and [`REFUSED`].

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{json, Map, Value};

use super::say::SayStreamer;
use super::{
    ChatRole, Completion, CompletionRequest, ExtraField, LlmError, LlmFailure, LlmProvider,
    TokenUsage, TransportInfo, Watcher,
};
use crate::ai::settings::{ThinkingEffort, ThinkingSettings};

/// What one endpoint has already refused, for as long as this window is open.
///
/// "OpenAI-compatible" is a claim about the route, not about the body. The
/// official API rejects a field it does not know with a 400, a gateway may ignore
/// it silently, and a local server may do either -- so which of these an endpoint
/// accepts is not something the user can be asked to know, and not something
/// worth writing to their config file. It is learned from the first refusal and
/// kept in memory: the cost of being wrong is one failed request per endpoint per
/// session, and the cost of asking would be a checkbox nobody can answer.
static REFUSED: LazyLock<Mutex<HashMap<String, HashSet<ExtraField>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The shared client. Connection pooling is the point: a task is a dozen requests
/// to the same host, and a fresh TLS handshake apiece is a second of nothing.
static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        // Silence is measured per read in `read_stream`; a whole-request timeout
        // would be the very thing that mistake was about.
        .build()
        .expect("a TLS backend is compiled in")
});

/// What one reply amounted to, however it arrived.
#[derive(Debug, Default)]
struct Answer {
    content: String,
    reasoning: String,
    finish_reason: Option<String>,
    /// Content-bearing frames. Always 1 for a body that was not a stream.
    frames: u32,
    /// Milliseconds to the first content, when there was a first to time.
    first_ms: Option<u64>,
}

pub struct HttpProvider {
    id: String,
    base_url: String,
    model: String,
    api_key: String,
    /// Whether the model is asked to think, and how hard. `show` is not this
    /// side's business.
    thinking: ThinkingSettings,
}

impl HttpProvider {
    pub fn new(base_url: &str, model: &str, api_key: &str, thinking: ThinkingSettings) -> Self {
        Self {
            id: format!("{model} @ {base_url}"),
            base_url: base_url.to_string(),
            model: model.to_string(),
            api_key: api_key.to_string(),
            thinking,
        }
    }

    fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        if base.ends_with("/chat/completions") {
            base.to_string()
        } else {
            format!("{base}/chat/completions")
        }
    }

    /// The fields beyond the common shape, asked for only where they change
    /// something.
    ///
    /// Thinking on at the usual strength is what an endpoint does when it is told
    /// nothing, so saying it adds a field that can be refused and buys no
    /// different answer. Saying the other thing does buy one, so it is said. What
    /// is left is the smallest request that still means what the user chose.
    fn extras(&self) -> Map<String, Value> {
        let mut extras = Map::new();
        if !self.thinking.enabled {
            extras.insert("thinking".into(), json!({ "type": "disabled" }));
        } else if self.thinking.effort != ThinkingEffort::High {
            extras.insert("reasoning_effort".into(), json!(self.thinking.effort.tag()));
        }
        if let Some(known) = REFUSED.lock().unwrap().get(&self.id) {
            for field in known {
                extras.remove(field.key());
            }
        }
        extras
    }

    fn body(&self, request: &CompletionRequest, extras: &Map<String, Value>) -> (Value, Value) {
        let mut params = Map::new();
        params.insert("model".into(), json!(self.model));
        // Ignored by a model that is thinking -- the reasoning endpoints say so
        // plainly -- and still the right thing to send to one that is not.
        params.insert("temperature".into(), json!(0));
        params.insert("stream".into(), json!(true));
        for (key, value) in extras {
            params.insert(key.clone(), value.clone());
        }
        // Without this an OpenAI-compatible stream carries no token counts at
        // all. Endpoints that do not know the option ignore it.
        params.insert("stream_options".into(), json!({ "include_usage": true }));

        let mut messages = vec![json!({ "role": "system", "content": request.system })];
        messages.extend(request.messages.iter().map(|message| {
            json!({
                "role": match message.role { ChatRole::User => "user", ChatRole::Assistant => "assistant" },
                "content": message.content,
            })
        }));

        let mut full = params.clone();
        full.insert("messages".into(), Value::Array(messages));
        (Value::Object(params), Value::Object(full))
    }

    async fn post(
        &self,
        request: &CompletionRequest,
        extras: &Map<String, Value>,
    ) -> Result<reqwest::Response, LlmError> {
        let (params, full) = self.body(request, extras);
        /*
         * The body as it goes out, and what this request adds to the
         * conversation. Together they answer "what did the model actually see",
         * which is the first question worth asking about a reply that made no
         * sense.
         *
         * The key is not part of it. It is a header, it never reaches the log,
         * and the one field here that could carry it does not exist.
         */
        request.watcher.logged_request(&json!({
            "endpoint": self.endpoint(),
            "params": params,
            "system": request.system,
            "messages": request.messages.iter().map(|message| json!({
                "role": match message.role { ChatRole::User => "user", ChatRole::Assistant => "assistant" },
                "content": message.content,
            })).collect::<Vec<_>>(),
        }));

        let mut builder = CLIENT
            .post(self.endpoint())
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream");
        if !self.api_key.is_empty() {
            builder = builder.bearer_auth(&self.api_key);
        }

        let send = builder.json(&full).send();
        tokio::select! {
            biased;
            () = request.cancel.cancelled() => Err(LlmError::new(LlmFailure::Cancelled, "cancelled")),
            result = send => result.map_err(|error| LlmError::new(LlmFailure::Network, error.to_string())),
        }
    }

    /// The whole-body shape, for an endpoint that ignored `stream`.
    async fn read_body(
        &self,
        response: reqwest::Response,
        watcher: &Arc<dyn Watcher>,
    ) -> Result<Answer, LlmError> {
        let text = response.text().await.unwrap_or_default();
        let payload: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        if let Some(message) = payload["error"]["message"].as_str() {
            return Err(LlmError::new(LlmFailure::Unknown, message));
        }

        report_usage(&payload["usage"], watcher);
        let choice = &payload["choices"][0];
        let content = choice["message"]["content"].as_str().unwrap_or_default().to_string();

        // Nothing was painted as it arrived, so the answer is handed over in one
        // piece. The panel cannot tell the difference and neither can the reader.
        if !content.is_empty() {
            let shown = SayStreamer::new().push(&content);
            if !shown.is_empty() {
                watcher.delta(&shown);
            }
        }
        Ok(Answer {
            content,
            reasoning: choice["message"]["reasoning_content"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            finish_reason: choice["finish_reason"].as_str().map(str::to_string),
            // One piece is one frame. Reporting it honestly is what lets the
            // panel say the answer did not stream instead of leaving the reader
            // to wonder.
            frames: 1,
            first_ms: None,
        })
    }

    /// Server-sent events, assembled a frame at a time.
    ///
    /// The transport guarantees nothing about where a chunk ends -- a frame can
    /// be split down the middle of a word or of a JSON escape -- so the boundary
    /// is found in the buffer rather than assumed at the edge of a read.
    async fn read_stream(
        &self,
        response: reqwest::Response,
        request: &CompletionRequest,
    ) -> Result<Answer, LlmError> {
        let watcher = &request.watcher;
        let mut stream = response.bytes_stream();
        let mut streamer = SayStreamer::new();
        let started = Instant::now();
        // Bytes rather than a String, because a chunk boundary can land inside a
        // multi-byte character and half of one is not text yet.
        let mut buffer: Vec<u8> = Vec::new();
        let mut answer = Answer::default();

        loop {
            /*
             * Every read gets the whole allowance, which is what makes this a
             * silence deadline rather than a duration one. A model reasoning its
             * way through a long context can take minutes before it writes
             * anything worth showing, and a clock started at the request calls
             * that a timeout, abandons it, and retries the whole thing -- slower,
             * twice the cost, and the user is told their working request failed.
             */
            let next = async {
                if request.timeout_ms == 0 {
                    stream.next().await.map(Ok)
                } else {
                    match tokio::time::timeout(
                        Duration::from_millis(request.timeout_ms),
                        stream.next(),
                    )
                    .await
                    {
                        Ok(item) => item.map(Ok),
                        Err(_) => Some(Err(())),
                    }
                }
            };

            let chunk = tokio::select! {
                biased;
                () = request.cancel.cancelled() => {
                    return Err(LlmError::new(LlmFailure::Cancelled, "cancelled"));
                }
                item = next => item,
            };

            let chunk = match chunk {
                None => break,
                Some(Err(())) => {
                    return Err(LlmError::new(
                        LlmFailure::Timeout,
                        format!("The endpoint sent nothing for {}ms.", request.timeout_ms),
                    ));
                }
                Some(Ok(Err(error))) => {
                    return Err(LlmError::new(LlmFailure::Network, error.to_string()));
                }
                Some(Ok(Ok(bytes))) => bytes,
            };
            buffer.extend_from_slice(&chunk);

            while let Some(cut) = buffer.iter().position(|byte| *byte == b'\n') {
                let line = String::from_utf8_lossy(&buffer[..cut]).trim().to_string();
                buffer.drain(..=cut);
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }

                // A frame that does not parse is one frame, not the end of the answer.
                let Ok(frame) = serde_json::from_str::<Value>(data) else { continue };
                if let Some(message) = frame["error"]["message"].as_str() {
                    return Err(LlmError::new(LlmFailure::Unknown, message));
                }

                report_usage(&frame["usage"], watcher);
                // The last frame of a stream carries usage and no choices at all.
                let choice = &frame["choices"][0];
                if choice.is_null() {
                    continue;
                }
                if let Some(reason) = choice["finish_reason"].as_str() {
                    answer.finish_reason = Some(reason.to_string());
                }

                let delta =
                    if choice["delta"].is_null() { &choice["message"] } else { &choice["delta"] };
                if let Some(thought) = delta["reasoning_content"].as_str() {
                    if !thought.is_empty() {
                        answer.reasoning.push_str(thought);
                        watcher.reasoning(thought);
                    }
                }
                let Some(text) = delta["content"].as_str().filter(|text| !text.is_empty()) else {
                    continue;
                };

                answer.frames += 1;
                if answer.first_ms.is_none() {
                    answer.first_ms = Some(started.elapsed().as_millis() as u64);
                }
                answer.content.push_str(text);
                let shown = streamer.push(text);
                if !shown.is_empty() {
                    watcher.delta(&shown);
                }
            }
        }

        Ok(answer)
    }
}

impl LlmProvider for HttpProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn describe(&self) -> String {
        format!("the model \"{}\", served from {}", self.model, self.base_url)
    }

    fn complete<'a>(&'a self, request: CompletionRequest) -> Completion<'a> {
        Box::pin(async move {
            let started = Instant::now();
            let extras = self.extras();
            let mut response = self.post(&request, &extras).await?;

            /*
             * A 400 with an optional field on board is the one failure worth
             * answering rather than reporting. Everything this client adds beyond
             * the common shape is a preference, and no preference is worth an
             * endpoint the user cannot talk to at all -- so it comes off and the
             * request goes again without it. Once: a second failure is about the
             * request itself and is reported as it stands.
             *
             * All of them come off together because `extras` never carries more
             * than one: the two fields are alternatives, `thinking` for off and
             * `reasoning_effort` for anything but the default strength.
             */
            let mut dropped: Vec<ExtraField> = extras
                .keys()
                .filter_map(|key| match key.as_str() {
                    "thinking" => Some(ExtraField::Thinking),
                    "reasoning_effort" => Some(ExtraField::ReasoningEffort),
                    _ => None,
                })
                .collect();
            dropped.sort();
            if response.status() == 400 && !dropped.is_empty() {
                // Read and discarded rather than ignored: a body nobody consumes
                // holds its socket until the process happens to notice.
                let _ = read_error(response).await;
                let second = self.post(&request, &Map::new()).await?;
                /*
                 * Only a second attempt that worked proves these fields were the
                 * problem. A 400 about the model name or the messages rejects
                 * both requests alike, and remembering a refusal on the strength
                 * of that would quietly turn a setting off for the rest of the
                 * session over something that had nothing to do with it.
                 */
                if second.status().is_success() {
                    let mut refused = REFUSED.lock().unwrap();
                    let known = refused.entry(self.id.clone()).or_default();
                    known.extend(dropped.iter().copied());
                    drop(refused);
                    request.watcher.degraded(&dropped);
                }
                response = second;
            }

            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string();

            if !response.status().is_success() {
                /*
                 * What the endpoint actually said, which is the whole of what the
                 * user can act on: "model not found", "insufficient balance",
                 * "this key has no access to that model". A bare status line
                 * sends them to the wrong place -- usually to their key, whatever
                 * the real reason was.
                 */
                let code = response.status();
                let detail = read_error(response).await;
                let status = format!("HTTP {}", code).trim().to_string();
                let message =
                    if detail.is_empty() { status } else { format!("{status}: {detail}") };
                return Err(LlmError::new(status_kind(code.as_u16()), message));
            }

            /*
             * What came back decides how it is read, which is the whole reason a
             * user can ask for a stream and not get one. An endpoint that ignores
             * `stream` answers with an ordinary JSON document and there is
             * nothing to be done about it here -- but there is something to be
             * said about it, which is what `transport` is for.
             */
            let streamed = content_type.contains("text/event-stream");
            let answer = if streamed {
                self.read_stream(response, &request).await?
            } else {
                self.read_body(response, &request.watcher).await?
            };
            let total_ms = started.elapsed().as_millis() as u64;
            request.watcher.transport(TransportInfo {
                streamed,
                frames: answer.frames,
                first_ms: answer.first_ms.unwrap_or(total_ms),
                total_ms,
            });
            /*
             * Both written whole, here, once, whether or not they are any use to
             * the caller -- a step that returned nothing usable is the one most
             * worth reading afterwards. Thinking first, because that is the order
             * it happened in.
             */
            if !answer.reasoning.is_empty() {
                request.watcher.logged_reasoning(&answer.reasoning);
            }
            request.watcher.logged_response(&answer.content);

            /*
             * `length` means the ceiling was hit mid-sentence, not that the model
             * had finished. Said out of band because the text is still worth
             * having: the caller decides whether a part-written answer is shown
             * with a note or thrown away, and it cannot decide that if this
             * arrives looking complete.
             */
            if answer.finish_reason.as_deref() == Some("length") {
                request.watcher.truncated();
            }

            if !answer.content.trim().is_empty() {
                return Ok(answer.content);
            }

            /*
             * A reasoning model can hand back an empty `content` with something in
             * the field next to it. Whether that something is the answer in the
             * wrong place or merely the thinking is not a question this layer can
             * answer -- so it is put to the caller, and a caller that says nothing
             * gets the honest empty.
             */
            if !answer.reasoning.trim().is_empty()
                && request.watcher.salvage_reasoning(&answer.reasoning)
            {
                return Ok(answer.reasoning);
            }

            // `finish_reason` is the difference between "ran out of room", "the
            // filter took it" and "no idea", and it costs nothing to carry.
            let why = match &answer.finish_reason {
                Some(reason) => format!(" (finish_reason: {reason})"),
                None => String::new(),
            };
            Err(LlmError::new(LlmFailure::Empty, format!("The model returned an empty reply.{why}")))
        })
    }
}

/// Only when the endpoint actually said, so a missing count is never shown as zero.
fn report_usage(usage: &Value, watcher: &Arc<dyn Watcher>) {
    let prompt = usage["prompt_tokens"].as_u64().unwrap_or(0) as u32;
    let completion = usage["completion_tokens"].as_u64().unwrap_or(0) as u32;
    if prompt > 0 || completion > 0 {
        watcher.usage(TokenUsage { prompt, completion });
    }
}

/// What the endpoint said went wrong, in one line.
///
/// Read as text and parsed afterwards, because a body that is not the JSON it
/// claimed to be is exactly the case worth reporting rather than swallowing -- an
/// HTML error page from a proxy says more about a 502 than any field would.
async fn read_error(response: reqwest::Response) -> String {
    let body = response.text().await.unwrap_or_default();
    if body.is_empty() {
        return String::new();
    }
    if let Ok(payload) = serde_json::from_str::<Value>(&body) {
        let message = payload["error"]["message"].as_str().or_else(|| payload["message"].as_str());
        if let Some(message) = message {
            return clip(message.trim(), 400);
        }
    }
    // Not JSON. The text itself is the message, which is the point.
    clip(&body.split_whitespace().collect::<Vec<_>>().join(" "), 400)
}

/// The first `limit` characters, cut on a character rather than a byte.
fn clip(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn status_kind(status: u16) -> LlmFailure {
    match status {
        401 | 403 => LlmFailure::Unauthorized,
        429 => LlmFailure::RateLimited,
        status if status >= 500 => LlmFailure::Network,
        _ => LlmFailure::Unknown,
    }
}

/// A provider that answers nothing, for a window with no endpoint configured.
///
/// Its own type rather than an `Option`, so every caller is spared a branch and
/// the one place that knows what "not configured" means says it once.
pub struct Unconfigured;

impl LlmProvider for Unconfigured {
    fn id(&self) -> &str {
        "unconfigured"
    }
    fn describe(&self) -> String {
        "a language model the user configured".to_string()
    }
    fn complete<'a>(&'a self, _request: CompletionRequest) -> Completion<'a> {
        Box::pin(async {
            Err(LlmError::new(LlmFailure::NotConfigured, "No model endpoint has been configured."))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::llm::ChatMessage;

    fn provider(thinking: ThinkingSettings) -> HttpProvider {
        HttpProvider::new("https://api.example.com/v1", "gpt-x", "sk-1", thinking)
    }

    #[test]
    fn the_route_is_appended_unless_it_is_already_there() {
        assert_eq!(
            provider(Default::default()).endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
        let direct = HttpProvider::new(
            "https://api.example.com/v1/chat/completions/",
            "m",
            "",
            Default::default(),
        );
        assert_eq!(direct.endpoint(), "https://api.example.com/v1/chat/completions");
    }

    #[test]
    fn the_default_thinking_settings_add_no_fields_at_all() {
        let extras = provider(ThinkingSettings::default()).extras();
        assert!(extras.is_empty(), "{extras:?}");
    }

    #[test]
    fn thinking_off_is_said_and_a_lower_effort_is_said() {
        let off = provider(ThinkingSettings { enabled: false, ..Default::default() });
        assert_eq!(off.extras()["thinking"], json!({ "type": "disabled" }));

        let low = provider(ThinkingSettings {
            effort: ThinkingEffort::Low,
            ..Default::default()
        });
        assert_eq!(low.extras()["reasoning_effort"], json!("low"));

        let max = provider(ThinkingSettings {
            effort: ThinkingEffort::Max,
            ..Default::default()
        });
        assert_eq!(max.extras()["reasoning_effort"], json!("max"));
    }

    #[test]
    fn a_remembered_refusal_takes_the_field_back_off() {
        let provider = HttpProvider::new("https://refuser.example", "m", "", ThinkingSettings {
            enabled: false,
            ..Default::default()
        });
        assert!(!provider.extras().is_empty());
        REFUSED
            .lock()
            .unwrap()
            .entry(provider.id.clone())
            .or_default()
            .insert(ExtraField::Thinking);
        assert!(provider.extras().is_empty());
    }

    #[test]
    fn the_request_body_carries_the_system_message_first() {
        let provider = provider(Default::default());
        let request = CompletionRequest {
            system: "be helpful".into(),
            messages: vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")],
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
        };
        let (params, full) = provider.body(&request, &Map::new());
        assert_eq!(params["model"], "gpt-x");
        assert_eq!(params["stream"], true);
        assert_eq!(params["stream_options"]["include_usage"], true);
        assert!(params.get("messages").is_none(), "the log's params exclude the conversation");

        let messages = full["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be helpful");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
    }

    #[test]
    fn statuses_map_to_the_failure_the_panel_can_act_on() {
        assert_eq!(status_kind(401), LlmFailure::Unauthorized);
        assert_eq!(status_kind(403), LlmFailure::Unauthorized);
        assert_eq!(status_kind(429), LlmFailure::RateLimited);
        assert_eq!(status_kind(502), LlmFailure::Network);
        assert_eq!(status_kind(400), LlmFailure::Unknown);
    }

    #[tokio::test]
    async fn an_unconfigured_endpoint_says_so_rather_than_failing_obscurely() {
        let request = CompletionRequest {
            system: String::new(),
            messages: Vec::new(),
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
        };
        let error = Unconfigured.complete(request).await.unwrap_err();
        assert_eq!(error.kind, LlmFailure::NotConfigured);
    }

    #[test]
    fn a_long_error_body_is_clipped_on_a_character() {
        assert_eq!(clip("你好世界", 2), "你好");
        assert_eq!(clip("short", 400), "short");
    }
}
