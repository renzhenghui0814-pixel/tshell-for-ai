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

use super::prose::ProseStreamer;
use super::{
    ChatMessage, ChatRole, Completion, CompletionRequest, ExtraField, LlmError, LlmFailure,
    LlmProvider, Reply, TokenUsage, ToolCall, ToolSpec, TransportInfo, Watcher,
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
    /// Assembled by index, because a stream sends one call in pieces across
    /// frames. See [`collect_calls`].
    calls: Vec<PartialCall>,
}

/// One tool call while it is still arriving.
///
/// `id` and `name` come in the first fragment for a given index and are absent
/// from every fragment after it; `arguments` arrives a few characters at a time
/// and is only valid JSON once the stream ends. So all three are accumulated
/// rather than read, and nothing is decided until the body is done.
#[derive(Debug, Default, Clone)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

impl PartialCall {
    fn finish(self) -> Option<ToolCall> {
        if self.name.trim().is_empty() {
            // A fragment stream that never named its function is not a call this
            // client can carry out, and inventing a name for it would run the
            // wrong one. Dropped here; the loop sees a reply with no calls and
            // nudges, which is the same path a mangled JSON object takes.
            return None;
        }
        Some(ToolCall {
            id: self.id,
            name: self.name,
            arguments: self.arguments,
        })
    }
}

/// Reads whatever tool calls a frame or a whole message carried into `into`.
///
/// One function for both shapes because they differ only in how complete each
/// entry is: a body carries `index` implicitly by position and every field at
/// once, a stream carries `index` explicitly and any subset of the fields. Both
/// are additive, so both are applied the same way.
fn collect_calls(value: &Value, into: &mut Vec<PartialCall>) {
    let Some(items) = value.as_array() else {
        return;
    };
    for (position, item) in items.iter().enumerate() {
        let index = item["index"]
            .as_u64()
            .map(|index| index as usize)
            .unwrap_or(position);
        if into.len() <= index {
            into.resize(index + 1, PartialCall::default());
        }
        let slot = &mut into[index];
        if let Some(id) = item["id"].as_str() {
            if !id.is_empty() {
                slot.id.push_str(id);
            }
        }
        if let Some(name) = item["function"]["name"].as_str() {
            if !name.is_empty() {
                slot.name.push_str(name);
            }
        }
        if let Some(arguments) = item["function"]["arguments"].as_str() {
            slot.arguments.push_str(arguments);
        }
    }
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
        // Tools are deliberately not here. `extras` is the settings the user
        // chose, and tools are neither a setting nor optional in the same sense:
        // they hang off the request and are governed by `with_tools`, which the
        // caller works out once and passes down.
        if let Some(known) = REFUSED.lock().unwrap().get(&self.id) {
            for field in known {
                extras.remove(field.key());
            }
        }
        extras
    }

    /// Whether this endpoint has already refused tools, for this window.
    fn tools_refused(&self) -> bool {
        REFUSED
            .lock()
            .unwrap()
            .get(&self.id)
            .is_some_and(|known| known.contains(&ExtraField::Tools))
    }

    /// The tools as the request wants them: OpenAI-shaped, one entry per spec.
    fn tool_field(tools: &[ToolSpec]) -> Value {
        Value::Array(
            tools
                .iter()
                .map(|tool| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": tool.name,
                            "description": tool.description,
                            "parameters": tool.parameters,
                        },
                    })
                })
                .collect(),
        )
    }

    fn body(
        &self,
        request: &CompletionRequest,
        extras: &Map<String, Value>,
        with_tools: bool,
    ) -> (Value, Value) {
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

        /*
         * `auto` rather than `required`. Answering without calling anything is
         * how a task ends here, so the model has to be free to do it -- and a
         * question that wanted a sentence deserves a sentence. Requiring a call
         * would leave no way to finish but to invent a step.
         */
        if with_tools && !request.tools.is_empty() {
            params.insert("tools".into(), Self::tool_field(&request.tools));
            params.insert("tool_choice".into(), json!("auto"));
        }

        let system = if with_tools {
            &request.system
        } else {
            &request.fallback_system
        };
        let mut messages = vec![json!({ "role": "system", "content": system })];
        messages.extend(Self::project_messages(&request.messages, with_tools));

        let mut full = params.clone();
        full.insert("messages".into(), Value::Array(messages));
        (Value::Object(params), Value::Object(full))
    }

    /// Projects the durable, structured history onto the protocol this endpoint
    /// understands. The stored history itself is never flattened.
    fn project_messages(messages: &[ChatMessage], with_tools: bool) -> Vec<Value> {
        messages
            .iter()
            .map(|message| {
                if !with_tools {
                    let mut content = message.content.clone();
                    for call in &message.tool_calls {
                        if !content.trim().is_empty() {
                            content.push('\n');
                        }
                        content.push_str(&Self::render_json_call(call));
                    }
                    return json!({
                        "role": if message.role == ChatRole::Assistant { "assistant" } else { "user" },
                        "content": Self::content_field(message, content),
                    });
                }

                match message.role {
                    ChatRole::User => json!({
                        "role": "user",
                        "content": Self::content_field(message, message.content.clone()),
                    }),
                    ChatRole::Tool => json!({
                        "role": "tool",
                        "tool_call_id": message.tool_call_id.as_deref().unwrap_or_default(),
                        "content": message.content,
                    }),
                    ChatRole::Assistant if !message.tool_calls.is_empty() => json!({
                        "role": "assistant",
                        "content": if message.content.is_empty() { Value::Null } else { json!(message.content) },
                        "tool_calls": message.tool_calls.iter().map(|call| json!({
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        })).collect::<Vec<_>>(),
                    }),
                    ChatRole::Assistant => {
                        json!({ "role": "assistant", "content": message.content })
                    }
                }
            })
            .collect()
    }

    /// DeepSeek's OpenAI-compatible vision shape. Text-only turns deliberately
    /// remain strings so every existing endpoint sees byte-for-byte the request
    /// shape it already accepts.
    fn content_field(message: &ChatMessage, text: String) -> Value {
        if message.role != ChatRole::User || message.images.is_empty() {
            return json!(text);
        }
        let mut blocks = Vec::with_capacity(message.images.len() + 1);
        if !text.is_empty() {
            blocks.push(json!({ "type": "text", "text": text }));
        }
        blocks.extend(message.images.iter().map(|image| {
            json!({
                "type": "image_url",
                "image_url": { "url": image.data_url() },
            })
        }));
        Value::Array(blocks)
    }

    /// Request logs explain that an image was present without copying megabytes
    /// of opaque data into every tool step's transcript.
    fn log_messages(mut messages: Vec<Value>) -> Vec<Value> {
        for message in &mut messages {
            let Some(blocks) = message.get_mut("content").and_then(Value::as_array_mut) else {
                continue;
            };
            for block in blocks {
                let Some(url) = block
                    .get_mut("image_url")
                    .and_then(|image| image.get_mut("url"))
                else {
                    continue;
                };
                let label = url
                    .as_str()
                    .and_then(|value| value.strip_prefix("data:"))
                    .and_then(|value| value.split(';').next())
                    .map(|kind| format!("[{kind} image data omitted]"))
                    .unwrap_or_else(|| "[image data omitted]".to_string());
                *url = json!(label);
            }
        }
        messages
    }

    fn render_json_call(call: &ToolCall) -> String {
        let mut value = serde_json::from_str::<Value>(&call.arguments)
            .unwrap_or_else(|_| json!({ "arguments": call.arguments }));
        if let Some(object) = value.as_object_mut() {
            object.insert("action".into(), json!(call.name.trim().to_lowercase()));
        }
        serde_json::to_string(&value).unwrap_or_else(|_| call.arguments.clone())
    }

    async fn post(
        &self,
        request: &CompletionRequest,
        extras: &Map<String, Value>,
        with_tools: bool,
    ) -> Result<reqwest::Response, LlmError> {
        let (params, full) = self.body(request, extras, with_tools);
        /*
         * The body as it goes out, and what this request adds to the
         * conversation. Together they answer "what did the model actually see",
         * which is the first question worth asking about a reply that made no
         * sense.
         *
         * The key is not part of it. It is a header, it never reaches the log,
         * and the one field here that could carry it does not exist.
         */
        /*
         * The tools by name, not by schema.
         *
         * The full array is a page of JSON Schema, identical on every request of
         * every task, and a log of a twenty-step task would be twenty copies of
         * it with the answers buried between them. What the reader of that log
         * needs to know is which tools were on the table -- and, more to the
         * point, whether any were, since an endpoint that refused them is the
         * first thing to check when a model starts writing JSON by hand.
         */
        let mut params = params;
        if let Some(tools) = params.get("tools").and_then(|tools| tools.as_array()) {
            let names: Vec<&str> = tools
                .iter()
                .filter_map(|tool| tool["function"]["name"].as_str())
                .collect();
            params["tools"] = json!(names);
        }

        let actual_system = if with_tools {
            &request.system
        } else {
            &request.fallback_system
        };
        let projected = Self::log_messages(Self::project_messages(&request.messages, with_tools));
        request.watcher.logged_request(&json!({
            "endpoint": self.endpoint(),
            "params": params,
            "system": actual_system,
            "messages": projected,
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
        let content = choice["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let mut calls = Vec::new();
        collect_calls(&choice["message"]["tool_calls"], &mut calls);

        // Nothing was painted as it arrived, so the answer is handed over in one
        // piece. The panel cannot tell the difference and neither can the reader.
        if !content.is_empty() {
            let shown = ProseStreamer::new().push(&content);
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
            calls,
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
        let mut streamer = ProseStreamer::new();
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
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }

                // A frame that does not parse is one frame, not the end of the answer.
                let Ok(frame) = serde_json::from_str::<Value>(data) else {
                    continue;
                };
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

                let delta = if choice["delta"].is_null() {
                    &choice["message"]
                } else {
                    &choice["delta"]
                };
                /*
                 * Before the `content` test below, and not subject to it: a frame
                 * carrying a fragment of a tool call has no content at all, and
                 * the `continue` further down would throw the fragment away.
                 * Nothing is painted for these -- a half-written argument list is
                 * not something to show anyone -- so they do not count as frames
                 * either.
                 */
                collect_calls(&delta["tool_calls"], &mut answer.calls);
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
    fn describe(&self) -> String {
        format!(
            "the model \"{}\", served from {}",
            self.model, self.base_url
        )
    }

    fn complete<'a>(&'a self, request: CompletionRequest) -> Completion<'a> {
        Box::pin(async move {
            let started = Instant::now();
            let extras = self.extras();
            let mut sent_tools = !request.tools.is_empty() && !self.tools_refused();
            let mut response = self.post(&request, &extras, sent_tools).await?;

            let optional: Vec<ExtraField> = extras
                .keys()
                .filter_map(|key| match key.as_str() {
                    "thinking" => Some(ExtraField::Thinking),
                    "reasoning_effort" => Some(ExtraField::ReasoningEffort),
                    _ => None,
                })
                .collect();
            /*
             * Diagnose optional fields before tools. If both came off in one
             * retry, a provider that rejected only `thinking` would be remembered
             * as having no tools and every later request would unnecessarily use
             * the text protocol.
             */
            if response.status() == 400 && !optional.is_empty() {
                let _ = read_error(response).await;
                let second = self.post(&request, &Map::new(), sent_tools).await?;
                if second.status().is_success() {
                    let mut refused = REFUSED.lock().unwrap();
                    let known = refused.entry(self.id.clone()).or_default();
                    known.extend(optional.iter().copied());
                    drop(refused);
                    request.watcher.degraded(&optional);
                }
                response = second;
            }

            /*
             * Only a 400 that survives the extra-field retry is evidence against
             * tools. This retry changes both halves together: removes `tools`
             * from the body and selects the JSON-only prompt/history projection.
             */
            if response.status() == 400 && sent_tools {
                let _ = read_error(response).await;
                let second = self.post(&request, &Map::new(), false).await?;
                if second.status().is_success() {
                    REFUSED
                        .lock()
                        .unwrap()
                        .entry(self.id.clone())
                        .or_default()
                        .insert(ExtraField::Tools);
                    request.watcher.degraded(&[ExtraField::Tools]);
                }
                response = second;
                sent_tools = false;
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
                let message = if detail.is_empty() {
                    status
                } else {
                    format!("{status}: {detail}")
                };
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

            let calls: Vec<ToolCall> = answer
                .calls
                .into_iter()
                .filter_map(PartialCall::finish)
                .collect();
            /*
             * Calls nobody offered are dropped rather than acted on: whatever the
             * endpoint is echoing, it is not this request. Dropped rather than
             * returned early, so that a reply which was ONLY those calls falls
             * through to the empty-reply path below and is retried -- returning
             * here would hand the loop a blank answer and end the task in silence.
             */
            let calls = if sent_tools { calls } else { Vec::new() };

            /*
             * Logged here rather than above, because the calls have to be settled
             * first and they are half of what was said. Written whether or not
             * the loop can use it: a step that returned nothing usable is the one
             * most worth reading afterwards.
             */
            request.watcher.logged_response(&Reply {
                text: answer.content.clone(),
                calls: calls.clone(),
            });

            // Calls with no prose is the ordinary shape of a working step, so the
            // emptiness test below has to count them.
            if !answer.content.trim().is_empty() || !calls.is_empty() {
                return Ok(Reply {
                    text: answer.content,
                    calls,
                });
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
                return Ok(Reply::text(answer.reasoning));
            }

            // `finish_reason` is the difference between "ran out of room", "the
            // filter took it" and "no idea", and it costs nothing to carry.
            let why = match &answer.finish_reason {
                Some(reason) => format!(" (finish_reason: {reason})"),
                None => String::new(),
            };
            Err(LlmError::new(
                LlmFailure::Empty,
                format!("The model returned an empty reply.{why}"),
            ))
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
        let message = payload["error"]["message"]
            .as_str()
            .or_else(|| payload["message"].as_str());
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
    fn describe(&self) -> String {
        "a language model the user configured".to_string()
    }
    fn complete<'a>(&'a self, _request: CompletionRequest) -> Completion<'a> {
        Box::pin(async {
            Err(LlmError::new(
                LlmFailure::NotConfigured,
                "No model endpoint has been configured.",
            ))
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
        assert_eq!(
            direct.endpoint(),
            "https://api.example.com/v1/chat/completions"
        );
    }

    #[test]
    fn the_default_thinking_settings_add_no_fields_at_all() {
        let extras = provider(ThinkingSettings::default()).extras();
        assert!(extras.is_empty(), "{extras:?}");
    }

    #[test]
    fn thinking_off_is_said_and_a_lower_effort_is_said() {
        let off = provider(ThinkingSettings {
            enabled: false,
            ..Default::default()
        });
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
        let provider = HttpProvider::new(
            "https://refuser.example",
            "m",
            "",
            ThinkingSettings {
                enabled: false,
                ..Default::default()
            },
        );
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
            fallback_system: "be helpful".into(),
            messages: vec![ChatMessage::user("hi"), ChatMessage::assistant("hello")],
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
            tools: Vec::new(),
        };
        let (params, full) = provider.body(&request, &Map::new(), false);
        assert_eq!(params["model"], "gpt-x");
        assert_eq!(params["stream"], true);
        assert_eq!(params["stream_options"]["include_usage"], true);
        assert!(
            params.get("messages").is_none(),
            "the log's params exclude the conversation"
        );

        let messages = full["messages"].as_array().unwrap();
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "be helpful");
        assert_eq!(messages[1]["role"], "user");
        assert_eq!(messages[2]["role"], "assistant");
    }

    #[test]
    fn vision_turns_use_text_and_image_url_blocks_without_changing_plain_turns() {
        let provider = provider(Default::default());
        let image = super::super::ChatImage {
            media_type: "image/png".into(),
            data: "iVBORw==".into(),
        };
        let request = CompletionRequest {
            system: "see".into(),
            fallback_system: "see".into(),
            messages: vec![
                ChatMessage::user_with_images("describe", vec![image]),
                ChatMessage::user("plain"),
            ],
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
            tools: Vec::new(),
        };

        let (_, body) = provider.body(&request, &Map::new(), false);
        let vision = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(vision[0], json!({ "type": "text", "text": "describe" }));
        assert_eq!(vision[1]["type"], "image_url");
        assert_eq!(
            vision[1]["image_url"]["url"],
            "data:image/png;base64,iVBORw=="
        );
        assert_eq!(body["messages"][2]["content"], "plain");

        let logged =
            HttpProvider::log_messages(HttpProvider::project_messages(&request.messages, false));
        assert_eq!(
            logged[0]["content"][1]["image_url"]["url"],
            "[image/png image data omitted]"
        );
    }

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: name.into(),
            description: "d".into(),
            parameters: json!({ "type": "object", "properties": {} }),
        }
    }

    #[test]
    fn tools_go_out_in_the_shape_the_endpoint_expects() {
        let provider = provider(Default::default());
        let request = CompletionRequest {
            system: String::new(),
            fallback_system: String::new(),
            messages: Vec::new(),
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
            tools: vec![spec("run"), spec("say")],
        };
        let (params, _) = provider.body(&request, &Map::new(), true);
        let tools = params["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "run");
        // `auto`, never `required`: a reply that calls nothing is how a task ends,
        // and a question that wanted a sentence deserves a sentence.
        assert_eq!(params["tool_choice"], "auto");
    }

    #[test]
    fn structured_history_is_native_until_tools_are_refused() {
        let provider = provider(Default::default());
        let call = ToolCall {
            id: "call_7".into(),
            name: "run".into(),
            arguments: r#"{"command":"uptime","why":"load"}"#.into(),
        };
        let request = CompletionRequest {
            system: "native prompt".into(),
            fallback_system: "json prompt".into(),
            messages: vec![
                ChatMessage::user("check"),
                ChatMessage::assistant_calls("", vec![call]),
                ChatMessage::tool_result("call_7", "uptime output"),
            ],
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
            tools: vec![spec("run")],
        };

        let (_, native) = provider.body(&request, &Map::new(), true);
        assert_eq!(native["messages"][0]["content"], "native prompt");
        assert_eq!(
            native["messages"][2]["tool_calls"][0]["function"]["name"],
            "run"
        );
        assert_eq!(native["messages"][3]["role"], "tool");
        assert_eq!(native["messages"][3]["tool_call_id"], "call_7");

        let (_, fallback) = provider.body(&request, &Map::new(), false);
        assert_eq!(fallback["messages"][0]["content"], "json prompt");
        assert_eq!(fallback["messages"][2]["role"], "assistant");
        assert_eq!(
            fallback["messages"][2]["content"],
            r#"{"action":"run","command":"uptime","why":"load"}"#
        );
        assert_eq!(fallback["messages"][3]["role"], "user");
        assert_eq!(fallback["messages"][3]["content"], "uptime output");
        assert!(fallback["messages"][2].get("tool_calls").is_none());
    }

    /// An endpoint that has refused tools once is not asked again this window --
    /// and the refusal is recorded by leaving the key in, not by removing it,
    /// which is backwards from the other two fields and worth pinning down.
    #[test]
    fn an_endpoint_that_refused_tools_is_not_offered_them_again() {
        // Its own endpoint name: `REFUSED` is process-wide and the tests run
        // together, so a refusal recorded against the shared one would leak into
        // whichever other test read it next.
        let provider = HttpProvider::new(
            "https://refused-tools.example/v1",
            "m",
            "",
            Default::default(),
        );
        assert!(!provider.tools_refused());
        REFUSED
            .lock()
            .unwrap()
            .entry(provider.id.clone())
            .or_default()
            .insert(ExtraField::Tools);
        assert!(provider.tools_refused());

        let request = CompletionRequest {
            system: String::new(),
            fallback_system: String::new(),
            messages: Vec::new(),
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
            tools: vec![spec("run")],
        };
        // What `complete` works out and passes down.
        let with_tools = !request.tools.is_empty() && !provider.tools_refused();
        let (params, _) = provider.body(&request, &provider.extras(), with_tools);
        assert!(params.get("tools").is_none());
        assert!(params.get("tool_choice").is_none());
        // The user's own settings are untouched by any of this.
        assert!(provider.extras().is_empty());
        REFUSED.lock().unwrap().remove(&provider.id);
    }

    /// The shape a stream actually sends: the id and the name once, then the
    /// arguments a few characters at a time, all keyed by index.
    #[test]
    fn a_tool_call_is_assembled_from_its_fragments() {
        let mut calls = Vec::new();
        collect_calls(
            &json!([{ "index": 0, "id": "call_a", "function": { "name": "run", "arguments": "{\"comm" } }]),
            &mut calls,
        );
        collect_calls(
            &json!([{ "index": 0, "function": { "arguments": "and\":\"df -h\"}" } }]),
            &mut calls,
        );

        let finished: Vec<_> = calls.into_iter().filter_map(PartialCall::finish).collect();
        assert_eq!(finished.len(), 1);
        assert_eq!(finished[0].id, "call_a");
        assert_eq!(finished[0].name, "run");
        assert_eq!(finished[0].arguments, r#"{"command":"df -h"}"#);
    }

    /// Two calls in one reply arrive interleaved by index, not in order.
    #[test]
    fn parallel_calls_are_kept_apart_by_index() {
        let mut calls = Vec::new();
        collect_calls(
            &json!([
                { "index": 0, "id": "a", "function": { "name": "run", "arguments": "{\"x\"" } },
                { "index": 1, "id": "b", "function": { "name": "say", "arguments": "{\"y\"" } }
            ]),
            &mut calls,
        );
        collect_calls(
            &json!([{ "index": 1, "function": { "arguments": ":2}" } }]),
            &mut calls,
        );
        collect_calls(
            &json!([{ "index": 0, "function": { "arguments": ":1}" } }]),
            &mut calls,
        );

        let finished: Vec<_> = calls.into_iter().filter_map(PartialCall::finish).collect();
        assert_eq!(finished.len(), 2);
        assert_eq!(finished[0].name, "run");
        assert_eq!(finished[0].arguments, r#"{"x":1}"#);
        assert_eq!(finished[1].name, "say");
        assert_eq!(finished[1].arguments, r#"{"y":2}"#);
    }

    /// A fragment stream that never named its function cannot be carried out, and
    /// inventing a name for it would run the wrong thing.
    #[test]
    fn a_call_with_no_name_is_dropped_rather_than_guessed_at() {
        let mut calls = Vec::new();
        collect_calls(
            &json!([{ "index": 0, "function": { "arguments": "{}" } }]),
            &mut calls,
        );
        assert!(calls
            .into_iter()
            .filter_map(PartialCall::finish)
            .next()
            .is_none());
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
            fallback_system: String::new(),
            messages: Vec::new(),
            timeout_ms: 0,
            cancel: Default::default(),
            watcher: Arc::new(super::super::Silent),
            tools: Vec::new(),
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
