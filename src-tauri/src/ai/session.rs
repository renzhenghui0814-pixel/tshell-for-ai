//! The plan-act-observe loop.
//!
//! It owns the conversation and the step budget and nothing else: executing a
//! command, judging one, and asking the user are all injected through
//! [`AgentHost`]. That keeps the whole control flow testable against fakes, which
//! matters here because the failure modes worth testing -- a refused command, a
//! declined confirmation, a timeout, running out of steps -- are awkward to
//! reproduce against a real host.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::cancel::Cancel;
use super::files::{FileOpError, FileRequest};
use super::host::{AgentConfig, AgentHost};
use super::llm::ToolCall;
use super::llm::{
    ChatMessage, CompletionRequest, LlmError, LlmFailure, Reply, TokenUsage, TransportInfo, Watcher,
};
use super::moves::TransferRequest;
use super::parse::{carries_action, parse_actions, ParsedReply};
use super::policy::command::Verdict;
use super::policy::path::PathVerdict;
use super::prompt::{
    describe_memory, describe_result, describe_skill_failure, estimate_tokens, fold_output,
    fold_skill, skill_header, skill_key_of, KEEP_RECENT, MAX_FACT_CHARS,
};
use super::store::memory::AppendOutcome;
use super::store::skill::MAIN_FILE;
use super::tools::{action_from_call, tool_specs};
use super::types::{
    ActionKind, AgentAction, AgentEvent, AgentMode, MemoryOutcome, MemoryScope, SkillOutcome,
    Unconfirmed,
};

/// What one request told the session while it was in flight.
///
/// The `Watcher` the transport calls is a separate object from the session
/// because it is shared into the request and outlives the borrow; what it
/// collects is read back here once the request has finished.
struct StepWatcher<H: AgentHost> {
    host: Arc<H>,
    billed: AtomicBool,
    cut_off: AtomicBool,
    log: Arc<super::store::log::LogSession>,
}

/// Render either a legacy text message or an OpenAI-style multimodal content
/// array for the human-readable model transcript. Image bytes have already
/// been replaced by a short label in the transport layer; keeping the blocks
/// separate here makes both the user's words and the image type visible.
fn logged_message_content(content: &serde_json::Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let Some(blocks) = content.as_array() else {
        return String::new();
    };
    blocks
        .iter()
        .filter_map(|block| match block["type"].as_str() {
            Some("text") => block["text"].as_str().map(str::to_string),
            Some("image_url") => block["image_url"]["url"].as_str().map(str::to_string),
            _ => None,
        })
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[test]
fn model_log_renders_multimodal_text_and_image_information() {
    let content = serde_json::json!([
        { "type": "text", "text": "describe this screenshot" },
        { "type": "image_url", "image_url": { "url": "[image/png image data omitted]" } }
    ]);
    assert_eq!(
        logged_message_content(&content),
        "describe this screenshot\n[image/png image data omitted]"
    );
}

impl<H: AgentHost + 'static> Watcher for StepWatcher<H> {
    fn delta(&self, text: &str) {
        self.host.emit(AgentEvent::Delta {
            text: text.to_string(),
        });
    }
    fn reasoning(&self, text: &str) {
        self.host.emit(AgentEvent::Reasoning {
            text: text.to_string(),
        });
    }
    fn usage(&self, usage: TokenUsage) {
        self.billed.store(true, Ordering::SeqCst);
        self.host.emit(AgentEvent::Usage {
            prompt: usage.prompt,
            completion: usage.completion,
            estimated: None,
        });
    }
    fn transport(&self, info: TransportInfo) {
        self.host.emit(AgentEvent::Transport {
            streamed: info.streamed,
            frames: info.frames,
            first_ms: info.first_ms,
            total_ms: info.total_ms,
        });
    }
    fn retry(&self, attempt: u32, kind: LlmFailure) {
        self.host.emit(AgentEvent::Retry {
            attempt,
            kind: kind.tag().to_string(),
        });
    }
    fn degraded(&self, fields: &[super::llm::ExtraField]) {
        self.host.emit(AgentEvent::Degraded {
            fields: fields.iter().map(|field| field.key().to_string()).collect(),
        });
    }
    /*
     * Only when the thinking is carrying an action. That is the whole of the case
     * the salvage was written for -- an answer written into the wrong field -- and
     * it is checked rather than assumed, because the far commoner reason for an
     * empty reply is a model that thought and never answered. Its thinking is a
     * plan in the first person: shown as a reply it repeats the card above it word
     * for word, and ends the task at the point the model was describing what it
     * would do next.
     */
    fn salvage_reasoning(&self, text: &str) -> bool {
        carries_action(text)
    }
    fn truncated(&self) {
        self.cut_off.store(true, Ordering::SeqCst);
    }
    fn logged_request(&self, body: &serde_json::Value) {
        let messages: Vec<(String, String)> = body["messages"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        (
                            item["role"].as_str().unwrap_or_default().to_string(),
                            logged_message_content(&item["content"]),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        self.log.request(&super::store::log::LogRequest {
            endpoint: body["endpoint"].as_str().unwrap_or_default(),
            params: &body["params"],
            system: body["system"].as_str().unwrap_or_default(),
            messages,
        });
    }
    fn logged_reasoning(&self, text: &str) {
        self.log.reasoning(text);
    }
    fn logged_response(&self, reply: &Reply) {
        // The same rendering the conversation itself gets, so what the log shows
        // the model asking for and what the next request carries are one thing.
        let text = &transcribe_reply(reply);
        self.log.response(text);
    }
}

/// One tool call written back out as the JSON object it stands for.
///
/// Serialised rather than formatted so that a value the model wrote -- a file
/// full of quotes and newlines -- survives into the history exactly. Arguments
/// that will not parse are kept as the text they were: this is a record of what
/// was asked for, and a record that silently dropped the unreadable part would be
/// the wrong record to read back when working out what went wrong.
fn render_call(call: &ToolCall) -> String {
    let verb = call.name.trim().to_lowercase();
    match super::parse::read_object(call.arguments.trim()) {
        Some(mut record) => {
            record.insert("action".into(), serde_json::Value::String(verb));
            serde_json::to_string(&serde_json::Value::Object(record))
                .unwrap_or_else(|_| call.arguments.clone())
        }
        None => format!(
            "{{\"action\":\"{verb}\",\"arguments\":{}}}",
            serde_json::to_string(&call.arguments).unwrap_or_else(|_| "\"\"".into())
        ),
    }
}

/// The reply as the conversation should remember it.
///
/// A tool call is written back out as the JSON object it is equivalent to, so
/// that a history read a week later shows the same thing whichever track produced
/// it -- and so that a task which started on one endpoint and continued on
/// another does not look like it changed language halfway.
///
/// A free function because two callers need it and only one of them has a
/// session: the loop, which puts this into the history, and the log's watcher,
/// which writes the same words into the transcript. Those two agreeing is the
/// point -- a log that disagreed with the context would be worse than no log.
fn transcribe_reply(reply: &Reply) -> String {
    if reply.calls.is_empty() {
        return reply.text.clone();
    }
    let mut out = reply.text.trim().to_string();
    for call in &reply.calls {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_call(call));
    }
    out
}

/// The transcript of one reply, for the log test that has no session to reach
/// through.
#[cfg(test)]
pub fn transcribe_for_test(reply: &Reply) -> String {
    transcribe_reply(reply)
}

/// One action named the way a label above its result should name it.
///
/// The verb plus the one field that says which of several similar calls this was:
/// three `run`s in a batch are only distinguishable by their commands.
fn describe_call(action: &AgentAction) -> String {
    let verb = action.action.map(|kind| kind.tag()).unwrap_or("action");
    let detail = action
        .command
        .clone()
        .or_else(|| action.path.clone())
        .or_else(|| action.name.clone())
        .or_else(|| action.paths.first().cloned())
        .unwrap_or_default();
    if detail.is_empty() {
        verb.to_string()
    } else {
        // Long enough to tell two commands apart, short enough that a batch of
        // labels is not itself a wall of context.
        let clipped: String = detail.chars().take(80).collect();
        format!("{verb} {clipped}")
    }
}

pub struct AgentSession<H: AgentHost> {
    host: Arc<H>,
    config: Mutex<AgentConfig>,
    messages: Mutex<Vec<ChatMessage>>,
    /// Which skill files are in the conversation right now, as `id::file`.
    ///
    /// Kept so a model that loses its place is told to scroll up rather than
    /// handed the same six pages twice. It shrinks again when [`Self::trim`] folds
    /// a body away -- at that point the text really is gone, and refusing to
    /// reload it would be the bug rather than the saving.
    loaded: Mutex<HashSet<String>>,
    /// Results being collected, while a step is carrying out more than one action.
    ///
    /// `None` outside a batch, which is every step on the JSON track and most of
    /// them on the tool track too. See [`Self::flush_batch`].
    gathering: Mutex<Option<Vec<String>>>,
    cancel: Mutex<Option<Cancel>>,
    running: AtomicBool,
    /// Swappable, because a conversation gets a transcript of its own and a
    /// panel may start several. See `Panel::new_chat`.
    log: Mutex<Arc<super::store::log::LogSession>>,
}

impl<H: AgentHost + 'static> AgentSession<H> {
    pub fn new(host: Arc<H>, config: AgentConfig) -> Self {
        Self {
            host,
            config: Mutex::new(config),
            messages: Mutex::new(Vec::new()),
            loaded: Mutex::new(HashSet::new()),
            gathering: Mutex::new(None),
            cancel: Mutex::new(None),
            running: AtomicBool::new(false),
            log: Mutex::new(Arc::new(super::store::log::LogSession::inactive())),
        }
    }

    /// Points this session at a different transcript, for the conversation that
    /// has just replaced the last one.
    pub fn set_log(&self, log: Arc<super::store::log::LogSession>) {
        *self.log.lock().unwrap() = log;
    }

    /// How the model should answer "what are you?", taken from whichever endpoint
    /// is currently wired in.
    pub fn describe_provider(&self) -> String {
        self.config.lock().unwrap().provider.describe()
    }

    /// The conversation as it stands, for tests that assert on what the model
    /// will be handed next -- which is where the difference between the two
    /// tracks actually shows up.
    #[cfg(test)]
    pub fn messages_for_test(&self) -> Vec<ChatMessage> {
        self.messages.lock().unwrap().clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    pub fn history(&self) -> Vec<ChatMessage> {
        self.messages.lock().unwrap().clone()
    }

    /// How much conversation this session is carrying, in characters.
    ///
    /// The same figure [`Self::trim`] measures against `context_budget`,
    /// deliberately: a number on the toolbar that counted something else would say
    /// the budget was fine while the folding had already started. The system
    /// prompt is not in it -- it is rebuilt for every request and belongs to no
    /// turn of the conversation, so counting it would move this number when
    /// nothing was said.
    pub fn context_chars(&self) -> usize {
        self.messages
            .lock()
            .unwrap()
            .iter()
            .map(ChatMessage::char_len)
            .sum()
    }

    /// Replaces the parts of the wiring that can change between tasks.
    pub fn configure(&self, patch: impl FnOnce(&mut AgentConfig)) {
        patch(&mut self.config.lock().unwrap());
    }

    pub fn stop(&self) {
        if let Some(cancel) = self.cancel.lock().unwrap().as_ref() {
            cancel.cancel();
        }
    }

    pub fn reset(&self) {
        self.stop();
        self.messages.lock().unwrap().clear();
        self.loaded.lock().unwrap().clear();
    }

    /// Puts a recorded conversation back, so a task carries on where it left off
    /// rather than starting again with the model knowing nothing about it.
    pub fn restore(&self, mut messages: Vec<ChatMessage>) {
        self.reset();
        // A process can stop after a call was persisted but before its result
        // was. Native APIs reject that incomplete pair, so close it explicitly
        // before the conversation is used again.
        let answered: HashSet<String> = messages
            .iter()
            .filter_map(|message| message.tool_call_id.clone())
            .collect();
        let pending: Vec<String> = messages
            .iter()
            .flat_map(|message| message.tool_calls.iter())
            .filter(|call| !answered.contains(&call.id))
            .map(|call| call.id.clone())
            .collect();
        for id in pending {
            messages.push(ChatMessage::tool_result(
                id,
                "The previous session ended before this tool returned a result. Re-check it if it is still needed.",
            ));
        }
        let mut loaded = self.loaded.lock().unwrap();
        for message in &messages {
            // A reopened conversation still has its skill bodies in it, so the
            // ones already on screen are known again rather than fetched a second
            // time. A folded body carries no header, so it is correctly not
            // counted.
            if let Some(key) = skill_key_of(&message.content) {
                loaded.insert(key);
            }
        }
        drop(loaded);
        *self.messages.lock().unwrap() = messages;
    }

    #[cfg(test)]
    pub async fn send(&self, user_text: &str) {
        self.send_with_images(user_text, Vec::new()).await;
    }

    pub async fn send_with_images(&self, user_text: &str, images: Vec<super::llm::ChatImage>) {
        if self.is_running() {
            return;
        }
        self.messages
            .lock()
            .unwrap()
            .push(ChatMessage::user_with_images(user_text, images));
        self.checkpoint();
        self.run_loop().await;
    }

    async fn run_loop(&self) {
        self.running.store(true, Ordering::SeqCst);
        let cancel = Cancel::new();
        *self.cancel.lock().unwrap() = Some(cancel.clone());

        let outcome = self.steps(&cancel).await;

        // A model failure knows what kind it is, and the panel has a translation
        // and a next step waiting for each kind, so the code is passed on rather
        // than the sentence.
        if let Err(error) = outcome {
            if cancel.is_cancelled() {
                self.host.emit(AgentEvent::Stopped);
            } else {
                self.host.emit(AgentEvent::Error {
                    message: error.message.clone(),
                    code: Some(error.kind.key().to_string()),
                });
            }
        }

        self.running.store(false, Ordering::SeqCst);
        *self.cancel.lock().unwrap() = None;
        self.host.emit(AgentEvent::Idle);
    }

    async fn steps(&self, cancel: &Cancel) -> Result<(), LlmError> {
        let mut repairs = 0;
        let mut blanks = 0;
        let max_steps = self.config.lock().unwrap().max_steps;
        // An unlimited task is the normal one: it ends when the work is done, when
        // the user stops it, or when the model gives up. The step limit is left in
        // for whoever wants a ceiling, and is off unless they ask for one.
        let mut step: u32 = 0;
        while max_steps == 0 || step < max_steps {
            if cancel.is_cancelled() {
                self.host.emit(AgentEvent::Stopped);
                return Ok(());
            }

            self.host.emit(AgentEvent::Thinking { step: step + 1 });
            self.trim();
            // After the folding rather than before it, so the figure is what the
            // next request actually carries and not what it would have carried.
            self.host.emit(AgentEvent::Context {
                chars: self.context_chars() as u32,
            });

            let watcher = Arc::new(StepWatcher {
                host: self.host.clone(),
                billed: AtomicBool::new(false),
                cut_off: AtomicBool::new(false),
                log: self.log.lock().unwrap().clone(),
            });
            let (system, fallback_system, timeout_ms) = {
                let config = self.config.lock().unwrap();
                (
                    config.system.clone(),
                    config.fallback_system.clone(),
                    config.request_timeout_ms,
                )
            };
            let request = CompletionRequest {
                system: system.clone(),
                fallback_system,
                messages: self.messages.lock().unwrap().clone(),
                timeout_ms,
                cancel: cancel.clone(),
                watcher: watcher.clone(),
                /*
                 * Offered every step, and offered to every endpoint. Whether they
                 * survive is the transport's business: one that rejects the field
                 * has the request sent again without it, remembers the refusal for
                 * the window, and the model answers in the JSON protocol the
                 * system prompt still describes. Nothing here has to know which
                 * track it got -- `reply.calls` is empty either way.
                 */
                tools: tool_specs(self.host.memory_enabled(), self.host.skills_enabled()),
            };

            // The handle is taken and the lock let go before the request starts:
            // it lasts as long as the model takes to answer, and `configure` must
            // not be blocked behind it.
            let provider = self.config.lock().unwrap().provider.clone();
            let reply = provider.complete(request).await;

            let mut reply = match reply {
                Ok(reply) => reply,
                Err(error) => {
                    // A 200 that carried no answer is a hiccup on the way, not a
                    // decision: asking the same question again usually gets one.
                    // Once only, so a provider that has stopped answering is not
                    // asked forever.
                    if error.kind == LlmFailure::Empty && blanks == 0 {
                        blanks += 1;
                        /*
                         * Said out loud, and for more than politeness. A reasoning
                         * model that answers nothing has usually just drawn a full
                         * card of thinking, and the panel drops that card when it
                         * is told the attempt failed. Left unsaid, the next
                         * attempt's thinking piles up under the last one's and the
                         * pause has no reason on screen.
                         */
                        self.host.emit(AgentEvent::Retry {
                            attempt: blanks,
                            kind: "empty".into(),
                        });
                        continue;
                    }
                    return Err(error);
                }
            };
            for (index, call) in reply.calls.iter_mut().enumerate() {
                if call.id.trim().is_empty() {
                    call.id = format!("call_{}_{}", step + 1, index + 1);
                }
            }
            blanks = 0;
            let cut_off = watcher.cut_off.load(Ordering::SeqCst);

            /*
             * Worked out here when the endpoint would not say. Counting is this
             * client's job either way -- what it sent is in front of it and so is
             * what came back -- and a number that is roughly right, labelled as
             * roughly right, beats a blank space where the cost should be.
             */
            if !watcher.billed.load(Ordering::SeqCst) {
                let prompt = estimate_tokens(&system)
                    + self
                        .messages
                        .lock()
                        .unwrap()
                        .iter()
                        .map(|message| estimate_tokens(&message.content))
                        .sum::<u32>();
                self.host.emit(AgentEvent::Usage {
                    prompt,
                    // The transcript rather than the prose: on the tool track the
                    // calls are most of what the model actually produced, and
                    // billing the sentence in front of them would report a
                    // fraction of the step.
                    completion: estimate_tokens(&transcribe_reply(&reply)),
                    estimated: Some(true),
                });
            }
            if cancel.is_cancelled() {
                self.host.emit(AgentEvent::Stopped);
                return Ok(());
            }
            /*
             * What went into the history, which is not always what came back.
             *
             * On the JSON track the two are the same text. On the tool track the
             * model's calls arrived beside its prose in a field of their own, and
             * a history holding only the prose would show the model announcing
             * work with no record of having asked for it -- so the calls are
             * written out as the objects they are equivalent to. Both tracks then
             * read back identically, days later, out of the chat store.
             */
            let remembered = if reply.calls.is_empty() {
                ChatMessage::assistant(&reply.text)
            } else {
                ChatMessage::assistant_calls(&reply.text, reply.calls.clone())
            };
            self.messages.lock().unwrap().push(remembered);
            self.checkpoint();

            // The reply as it was written is already in the log, put there by the
            // provider that received it. What this client made of it is visible in
            // the thread, and in what the next request carries.
            let ParsedReply {
                actions,
                unreadable,
                prose,
            } = self.read_reply(&reply);

            /*
             * A reply asking for nothing is the answer, and the task is over.
             *
             * Both tracks, for the same reason: the model had a channel for acting
             * and did not use it, so what it wrote is what it meant. The nudge that
             * used to follow prose on the JSON track cost a whole extra request and
             * produced a second card under an answer the user had already read --
             * it was telling the model "NOTHING happened" while drawing that same
             * reply to the user as the answer, and one of those two had to be wrong.
             */
            if actions.is_empty() && !unreadable {
                self.host.emit(AgentEvent::Reply { text: prose });
                /*
                 * A reply that ran out of room is shown and then said to be short.
                 * Not retried: the ceiling that stopped it is still there. Not
                 * hidden either -- the text stops mid-sentence, and a reader who is
                 * not told why is left thinking the assistant froze.
                 */
                if cut_off {
                    self.emit_truncated();
                }
                return Ok(());
            }

            /*
             * A reply whose action would not parse runs nothing at all.
             *
             * The prose before it is not the answer either: the model wrote it on
             * the way to doing something, and the something is what was lost. So
             * the step is spent asking for the action again rather than drawing
             * half a turn and stopping.
             */
            if unreadable {
                repairs += 1;
                if repairs > 1 {
                    self.host.emit(AgentEvent::Error {
                        message: "The model did not reply with a readable action, twice in a row."
                            .into(),
                        code: Some("protocol".into()),
                    });
                    return Ok(());
                }
                let nudge = protocol_nudge(cut_off);
                if reply.calls.is_empty() {
                    self.observe(&nudge);
                } else {
                    self.push_tool_results(&reply.calls, vec![nudge; reply.calls.len()]);
                }
                step += 1;
                continue;
            }

            // A run of bad replies is over: something parsed and is about to be
            // carried out.
            repairs = 0;

            /*
             * A batch gathers its results and reports them as one turn.
             *
             * Every action pushes an observation, and a batch of three pushed
             * three user messages in a row -- which reads to the model as the user
             * having spoken three times, and leaves the results with nothing
             * saying which call each belonged to. So for a batch the observations
             * are collected and joined, labelled by call. A single action is
             * untouched by any of this and still pushes its result the moment it
             * has one.
             */
            let batch = actions;
            if !reply.calls.is_empty() || batch.len() > 1 {
                *self.gathering.lock().unwrap() = Some(Vec::new());
            }

            /*
             * What the model said on its way to acting, shown before the card it
             * drew. Once for the step rather than once per action: it is one
             * sentence about everything that follows.
             */
            if !prose.is_empty() {
                self.host.emit(AgentEvent::Reply { text: prose });
            }

            for action in &batch {
                if cancel.is_cancelled() {
                    self.flush_batch(&batch, &reply.calls);
                    self.host.emit(AgentEvent::Stopped);
                    return Ok(());
                }

                let kind = action.action.expect("a verb, or it would not be an action");
                if kind.as_file_op().is_some() {
                    self.run_file_action(action, cancel).await;
                } else if kind == ActionKind::Remember {
                    self.run_memory_action(action).await;
                } else if kind == ActionKind::Skill {
                    self.run_skill_action(action).await;
                } else if kind.as_transfer().is_some() {
                    self.run_transfer_action(action, cancel).await;
                } else {
                    self.run_command_action(action, cancel).await;
                }
            }

            self.flush_batch(&batch, &reply.calls);
            step += 1;
        }

        self.host.emit(AgentEvent::StepLimit { steps: max_steps });
        Ok(())
    }

    fn emit_truncated(&self) {
        self.host.emit(AgentEvent::Error {
            message: "The model ran out of room and the reply above stops mid-sentence.".into(),
            code: Some("truncatedReply".into()),
        });
    }

    async fn run_command_action(&self, action: &AgentAction, cancel: &Cancel) {
        let command = action
            .command
            .clone()
            .unwrap_or_default()
            .trim()
            .to_string();
        if command.is_empty() {
            self.observe("The run action needs a non-empty command.");
            return;
        }

        let policy = self.host.classify(&command);
        let why = action.why.clone().unwrap_or_default();

        if policy.verdict == Verdict::Refuse {
            let reasons = self.host.describe_risk(&policy.reasons);
            self.host.emit(AgentEvent::Refused {
                command: command.clone(),
                reasons,
            });
            let keys: Vec<&str> = policy.reasons.iter().map(|reason| reason.key()).collect();
            self.observe(&[
                format!("REFUSED and not run: blocked as destructive ({}).", keys.join(", ")),
                "Only a few catastrophic shapes are blocked. A narrower command that affects one"
                    .into(),
                "process, service or file is allowed, and the user is simply asked to confirm it."
                    .into(),
                "Propose that narrower command now. Do not stop, and do not ask permission to retry."
                    .into(),
            ]
            .join("\n"));
            return;
        }

        /*
         * `auto` answers the confirmation instead of the user.
         *
         * Only the confirmation. The refusal above has already had its say and is
         * not reached from here, so the catastrophic shapes stay blocked in this
         * mode exactly as they are in the other two. What the user turned off is
         * being asked, not the floor underneath the asking.
         */
        let unconfirmed = (policy.verdict == Verdict::Confirm
            && self.host.mode() == AgentMode::Auto)
            .then_some(Unconfirmed::Auto);
        self.host.emit(AgentEvent::Command {
            command: command.clone(),
            why: why.clone(),
            verdict: policy.verdict,
            blocker: policy.blocker.clone(),
            unconfirmed,
        });

        if policy.verdict == Verdict::Confirm
            && unconfirmed.is_none()
            && !self.host.confirm_command(&command, &why, &policy).await
        {
            self.host.emit(AgentEvent::Declined { command });
            self.observe(
                "The user declined to run that command. Propose a different approach, or ask the \
                 user what to do.",
            );
            return;
        }
        if cancel.is_cancelled() {
            self.host.emit(AgentEvent::Stopped);
            return;
        }

        let result = self.host.execute(&command, cancel).await;
        self.host.emit(AgentEvent::Result {
            exit_code: result.exit_code,
            output: result.output.clone(),
            timed_out: result.timed_out,
            truncated: result.truncated,
        });
        self.observe(&describe_result(&result));
    }

    /// One step of the loop, for the three actions that change a file.
    ///
    /// The sequence is the same one a command goes through -- judge, show, ask, do
    /// -- with the judging split in two: where the bytes may land is decided from
    /// the path alone and offline, and what they will do to the file is decided by
    /// reading it. Only the second needs the machine, and it is why the plan
    /// exists: the user is asked about a change that has already been resolved
    /// against the file as it actually is, not about the model's description of one.
    async fn run_file_action(&self, action: &AgentAction, cancel: &Cancel) {
        let kind = action
            .action
            .and_then(ActionKind::as_file_op)
            .expect("a file verb");
        let path = action.path.clone().unwrap_or_default().trim().to_string();
        let why = action.why.clone().unwrap_or_default();
        if path.is_empty() {
            self.observe(&format!(
                "The {} action needs a \"path\". Send it again with the absolute path of the file.",
                kind.tag()
            ));
            return;
        }

        let policy = self.host.classify_path(&path);
        if policy.verdict == PathVerdict::Refuse {
            let reasons = self.host.describe_risk(&policy.reasons);
            self.host.emit(AgentEvent::Refused {
                command: format!("{} {path}", kind.tag()),
                reasons,
            });
            let keys: Vec<&str> = policy.reasons.iter().map(|reason| reason.key()).collect();
            self.observe(&[
                format!(
                    "REFUSED and not written: {path} is not somewhere a file may be written ({}).",
                    keys.join(", ")
                ),
                "Ordinary files anywhere else are allowed, with the user confirming. Pick another \
                 path,"
                    .into(),
                "or do it with a command if it really is a device or kernel interface you need."
                    .into(),
            ]
            .join("\n"));
            return;
        }

        /*
         * Whether this one will be asked about, settled before anything is drawn.
         *
         * `auto` skips every dialog. `trust` skips this one only when the directory
         * holding the file is one the user has already said yes to -- asked of the
         * path the model sent rather than of the plan's, because they are the same
         * string and this has to be known before the plan exists.
         */
        let mode = self.host.mode();
        let unconfirmed = match mode {
            AgentMode::Auto => Some(Unconfirmed::Auto),
            AgentMode::Trust if self.host.is_trusted(&path).await => Some(Unconfirmed::Trusted),
            _ => None,
        };

        // Drawn before the machine is touched, so a plan that fails still has a
        // card to report itself in rather than vanishing.
        let label = format!("{} {path}", kind.tag());
        self.host.emit(AgentEvent::Command {
            command: label.clone(),
            why: why.clone(),
            verdict: Verdict::Confirm,
            blocker: None,
            unconfirmed,
        });

        let request = FileRequest {
            kind,
            path: path.clone(),
            content: action.content.clone(),
            old_text: action.old_text.clone(),
            new_text: action.new_text.clone(),
        };
        let plan = match self.host.plan_file(request, cancel).await {
            Ok(plan) => plan,
            Err(FileOpError(message)) => {
                self.host.emit(AgentEvent::Result {
                    exit_code: 1,
                    output: message.clone(),
                    timed_out: false,
                    truncated: false,
                });
                self.observe(&format!("The {} did not happen: {message}", kind.tag()));
                return;
            }
        };
        if cancel.is_cancelled() {
            return;
        }

        // The thread keeps what the dialog only showed once, so the change is
        // still there to read after it has been answered.
        self.host.emit(AgentEvent::File {
            kind,
            path: plan.path.clone(),
            exists: plan.exists,
            size: super::files::format_bytes(plan.bytes),
            preview: plan.preview.clone(),
            before: plan.before.clone().unwrap_or_default(),
            after: plan.after.clone().unwrap_or_default(),
            encoding: plan.encoding,
            line: plan.line,
        });

        if unconfirmed.is_none() && !self.host.confirm_file(&plan, &why).await {
            self.host.emit(AgentEvent::Declined { command: label });
            self.observe(
                "The user declined that file change. Propose a different approach, or ask them \
                 what to do.",
            );
            return;
        }
        if cancel.is_cancelled() {
            return;
        }

        let result = self.host.apply_file(&plan, cancel).await;
        self.host.emit(AgentEvent::Result {
            exit_code: result.exit_code,
            output: result.output.clone(),
            timed_out: result.timed_out,
            truncated: result.truncated,
        });
        self.observe(&describe_result(&result));
    }

    /// One step of the loop, for the two actions that move files between machines.
    ///
    /// The one step that is never confirmed. What makes that safe to leave
    /// unattended is the conflict rule underneath: an existing file is skipped,
    /// never replaced, so nothing that was already there can be lost while nobody
    /// is looking.
    ///
    /// The destination is judged twice. Once here, on what the model asked for --
    /// offline, before either machine is touched. Then again on the resolved plan,
    /// because a dismissed dialog and a chosen folder both come back through
    /// `plan_transfer`, and the path that lands in the plan may be neither the one
    /// the model sent nor one it has ever seen.
    async fn run_transfer_action(&self, action: &AgentAction, cancel: &Cancel) {
        let kind = action
            .action
            .and_then(ActionKind::as_transfer)
            .expect("a transfer verb");
        let paths: Vec<String> = action
            .paths
            .iter()
            .filter(|path| !path.is_empty())
            .cloned()
            .collect();
        let to = action.to.clone().unwrap_or_default().trim().to_string();
        let why = action.why.clone().unwrap_or_default();

        if !to.is_empty() && !self.refuse_target(kind, &to) {
            return;
        }

        let arrow = if to.is_empty() {
            String::new()
        } else {
            format!("-> {to}")
        };
        let label = [kind_tag(kind).to_string(), paths.join(" "), arrow]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        // Drawn before either machine is touched, so a plan that fails still has a
        // card to report itself in rather than vanishing.
        self.host.emit(AgentEvent::Command {
            command: label,
            why,
            verdict: Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });

        let request = TransferRequest { kind, paths, to };
        let plan = match self.host.plan_transfer(request, cancel).await {
            Ok(plan) => plan,
            Err(error) => {
                self.host.emit(AgentEvent::Result {
                    exit_code: 1,
                    output: error.0.clone(),
                    timed_out: false,
                    truncated: false,
                });
                self.observe(&format!(
                    "The {} did not happen: {}",
                    kind_tag(kind),
                    error.0
                ));
                return;
            }
        };
        if cancel.is_cancelled() {
            return;
        }
        if !self.refuse_target(kind, &plan.target) {
            return;
        }

        self.host.emit(AgentEvent::Transfer {
            kind,
            sources: plan.roots.iter().map(|root| root.path.clone()).collect(),
            target: plan.target.clone(),
        });

        let result = self.host.run_transfer(&plan, cancel).await;
        self.host.emit(AgentEvent::Result {
            exit_code: result.exit_code,
            output: result.output.clone(),
            timed_out: result.timed_out,
            truncated: result.truncated,
        });
        self.observe(&describe_result(&result));
    }

    /// Whether bytes may land there. False means the step is over: the refusal has
    /// already been drawn and reported.
    ///
    /// Only the destination is asked about. The source is read, and reading /proc
    /// is what half the read-only allowlist does.
    fn refuse_target(&self, kind: super::types::TransferKind, target: &str) -> bool {
        let policy = self.host.classify_path(target);
        if policy.verdict != PathVerdict::Refuse {
            return true;
        }
        let reasons = self.host.describe_risk(&policy.reasons);
        self.host.emit(AgentEvent::Refused {
            command: format!("{} -> {target}", kind_tag(kind)),
            reasons,
        });
        let keys: Vec<&str> = policy.reasons.iter().map(|reason| reason.key()).collect();
        self.observe(&[
            format!(
                "REFUSED and nothing was transferred: {target} is not somewhere files may be written"
            ),
            format!("({}). Ordinary directories anywhere else are allowed.", keys.join(", ")),
            "Pick another destination.".into(),
        ]
        .join("\n"));
        false
    }

    /// One step of the loop, for the action that files a durable fact.
    ///
    /// The shortest step there is: no machine, no plan, no confirmation. It is
    /// worth being a step at all rather than something inferred afterwards because
    /// the model decides what is worth keeping while it still has the reason in
    /// front of it, and because the result has to come back -- a full scope is
    /// something it must act on.
    ///
    /// There is no verb for the other direction. A line that stopped being true is
    /// the user's to drop, in the memory panel, where they can see everything that
    /// is filed about a machine at once -- a model deleting the user's own notes on
    /// its own initiative is a change nobody is watching being made.
    async fn run_memory_action(&self, action: &AgentAction) {
        let scope = action.scope.unwrap_or(MemoryScope::Server);
        let text = action.text.clone().unwrap_or_default();

        if !self.host.memory_enabled() {
            self.observe(
                "Memory is switched off for this setup, so nothing was stored. Carry on without                  it, and do not try again.",
            );
            return;
        }
        /*
         * A ceiling on one line, because the prompt's rule about it has already
         * been shown not to hold on its own. Length is the one signal for a
         * transcribed paragraph that cannot misread a genuine fact, and refusing
         * is safe either way, because the answer sends the model back to write a
         * shorter one rather than stopping the task.
         */
        let length = text.chars().count();
        if length > MAX_FACT_CHARS {
            self.host.emit(AgentEvent::Memory {
                scope,
                text: text.clone(),
                outcome: MemoryOutcome::Oversize,
                token: None,
            });
            self.observe(&[
                format!(
                    "NOT stored: that line is {length} characters and a remembered fact must be under"
                ),
                format!(
                    "{MAX_FACT_CHARS}. A line that long is a piece of command output written out,                      not a fact"
                ),
                "about how this setup is put together. Either boil it down to one short sentence                  that"
                    .into(),
                "would still be true if you had run nothing today, or drop it and put it in your                  answer"
                    .into(),
                "to the user instead. Do not send this line again. This does not block the task."
                    .into(),
            ]
            .join("
"));
            return;
        }

        let result = self.host.remember(scope, &text).await;
        let outcome = match result.outcome {
            AppendOutcome::Ok => MemoryOutcome::Ok,
            AppendOutcome::Duplicate => MemoryOutcome::Duplicate,
            AppendOutcome::Full => MemoryOutcome::Full,
            AppendOutcome::Failed => MemoryOutcome::Failed,
        };
        self.host.emit(AgentEvent::Memory {
            scope,
            text,
            outcome,
            token: result.token,
        });
        self.observe(&describe_memory(scope, outcome));
    }

    /// One step of the loop, for the action that pulls a written procedure in.
    ///
    /// Local, read-only, and unconfirmed: this is the user's own text being read
    /// off the user's own disk, and nothing about the machine changes because of
    /// it. The body goes in under a header [`Self::trim`] can find later.
    async fn run_skill_action(&self, action: &AgentAction) {
        let id = action.name.clone().unwrap_or_default().trim().to_string();
        let file = action.file.clone().unwrap_or_default().trim().to_string();
        let file = if file.is_empty() { None } else { Some(file) };

        if !self.host.skills_enabled() {
            // Reached only when the model invents the action, since it is not in
            // the prompt otherwise. Still drawn: a step that did nothing needs to
            // say so, or the thread has a gap in it that nothing accounts for.
            self.host.emit(AgentEvent::Skill {
                id: if id.is_empty() { "?".into() } else { id },
                file: file.unwrap_or_else(|| MAIN_FILE.into()),
                outcome: SkillOutcome::Off,
                truncated: None,
            });
            self.observe(
                "[tshell client notice -- not from the user. Do not reply to this message and do \
                 not apologise.] Skills are switched off for this setup, so nothing was read. \
                 Carry on with the task using what you already know, and do not try this action \
                 again.",
            );
            return;
        }
        if id.is_empty() {
            self.observe(
                "The skill action needs a \"name\". Send it again naming one of the skills listed \
                 for you.",
            );
            return;
        }

        let asked = file.clone().unwrap_or_else(|| MAIN_FILE.to_string());
        let key = format!("{id}::{asked}");
        if self.loaded.lock().unwrap().contains(&key) {
            self.host.emit(AgentEvent::Skill {
                id: id.clone(),
                file: asked,
                outcome: SkillOutcome::Repeat,
                truncated: None,
            });
            self.observe(&format!(
                "[tshell client notice -- not from the user.] You have already loaded {} in this \
                 conversation and its text is still above. Scroll back to it rather than loading \
                 it again, and carry on with the task.",
                key.replace("::", " / ")
            ));
            return;
        }

        let result = self.host.load_skill(&id, file.as_deref()).await;
        let read_file = result.file.clone().unwrap_or_else(|| MAIN_FILE.to_string());
        self.host.emit(AgentEvent::Skill {
            id: id.clone(),
            file: read_file.clone(),
            outcome: result.outcome,
            truncated: result.truncated.then_some(true),
        });

        let Some(content) = result
            .content
            .filter(|_| result.outcome == SkillOutcome::Ok)
        else {
            self.observe(&describe_skill_failure(result.outcome, &id, &read_file));
            return;
        };

        self.loaded
            .lock()
            .unwrap()
            .insert(format!("{id}::{read_file}"));
        self.observe(
            &[
                skill_header(&id, &read_file),
                content,
                String::new(),
                "[End of the skill. It was written for this setup by the user or their team, so \
                 where it"
                    .into(),
                "disagrees with your own habits, follow it. It is a procedure, not a machine you \
                 have read:"
                    .into(),
                "verify each step against what the machine actually says. Carry on with the task \
                 now --"
                    .into(),
                "do not summarise what you have just read back to the user.]".into(),
            ]
            .join("\n"),
        );
    }

    /// Feeds an observation back as the next user turn, which is what drives the
    /// loop.
    fn observe(&self, text: &str) {
        // Mid-batch this is held rather than pushed; `flush_batch` sends the lot
        // as one turn. Outside a batch -- which is every step on the JSON track --
        // nothing about this changed.
        if let Some(gathering) = self.gathering.lock().unwrap().as_mut() {
            gathering.push(text.to_string());
            return;
        }
        self.messages.lock().unwrap().push(ChatMessage::user(text));
        self.checkpoint();
    }

    /// Persists exactly what the next model request would receive. This is kept
    /// beside every history mutation so closing the app mid-task cannot leave a
    /// visually complete chat whose model context is empty.
    fn checkpoint(&self) {
        self.host.checkpoint(&self.history());
    }

    /// Turns a batch's gathered results into the one turn the model reads.
    ///
    /// Labelled by call, because a model that asked three things at once and got
    /// three answers in a row has no other way to tell which is which -- and
    /// guessing wrongly about that is worse than not having asked in parallel at
    /// all. Idempotent: a step that was never a batch has nothing gathered and
    /// this does nothing.
    fn flush_batch(&self, batch: &[AgentAction], calls: &[ToolCall]) {
        let Some(results) = self.gathering.lock().unwrap().take() else {
            return;
        };
        if results.is_empty() {
            return;
        }
        if !calls.is_empty() {
            self.push_tool_results(calls, results);
            return;
        }
        let total = results.len();
        let joined = results
            .into_iter()
            .enumerate()
            .map(|(index, result)| {
                let what = batch
                    .get(index)
                    .map(describe_call)
                    .unwrap_or_else(|| "action".to_string());
                format!("[{} of {total} -- {what}]\n{result}", index + 1)
            })
            .collect::<Vec<_>>()
            .join("\n\n");
        self.observe(&joined);
    }

    fn push_tool_results(&self, calls: &[ToolCall], results: Vec<String>) {
        let mut messages = self.messages.lock().unwrap();
        for (index, call) in calls.iter().enumerate() {
            let content = results.get(index).cloned().unwrap_or_else(|| {
                "This tool was not run because the step stopped before it was reached.".into()
            });
            messages.push(ChatMessage::tool_result(&call.id, content));
        }
        drop(messages);
        self.checkpoint();
    }

    /// What one reply amounted to, whichever track it came back on.
    fn read_reply(&self, reply: &Reply) -> ParsedReply {
        if reply.calls.is_empty() {
            return parse_actions(&reply.text);
        }

        let actions: Vec<AgentAction> = reply.calls.iter().filter_map(action_from_call).collect();
        ParsedReply {
            /*
             * One call this build cannot read stops the whole batch, the same way
             * one unreadable object does on the other track. A model that said
             * "read this, then change that" and lost the reading must not have the
             * change carried out on its own.
             */
            unreadable: actions.len() != reply.calls.len(),
            actions,
            // Whatever it wrote alongside the calls. On this track it is ordinary
            // prose rather than something taken off the end of an object.
            prose: reply.text.trim().to_string(),
        }
    }

    /// Folds the oldest command output away once the conversation outgrows its
    /// budget.
    ///
    /// The conversation only ever grew before, and every step resent all of it, so
    /// a long task cost the square of its length. What it is mostly made of is
    /// command output, and the oldest of that is the most disposable thing in the
    /// request: the model read it, acted on it, and the consequence is further
    /// down.
    ///
    /// So the shape of the task is kept and the bulk is dropped. Two things are
    /// never touched: the first turn, which is the task itself, and the last few
    /// steps, which are what the next decision is actually made from.
    fn trim(&self) {
        let budget = self.config.lock().unwrap().context_budget;
        if budget == 0 {
            return;
        }

        let mut messages = self.messages.lock().unwrap();
        let mut total: usize = messages
            .iter()
            .map(|message| message.content.chars().count())
            .sum();
        if total <= budget {
            return;
        }
        let last = messages.len().saturating_sub(KEEP_RECENT);

        /*
         * Command output first, and only then a skill body.
         *
         * Both can be had again -- one by running the command, one by loading the
         * skill -- so neither is lost by folding. But output is a machine's answer
         * to a question already answered, while a skill is a procedure someone sat
         * down and wrote for this setup, and it is steering the whole task rather
         * than one step of it.
         */
        let folders: [fn(&str) -> Option<String>; 2] = [fold_output, fold_skill];
        for fold in folders {
            let mut index = 1;
            while index < last && total > budget {
                let message = &messages[index];
                if !matches!(
                    message.role,
                    super::llm::ChatRole::User | super::llm::ChatRole::Tool
                ) {
                    index += 1;
                    continue;
                }
                // A folded message no longer carries the header, so it is never
                // folded twice and the scan settles at the oldest thing still
                // worth dropping.
                let Some(folded) = fold(&message.content) else {
                    index += 1;
                    continue;
                };
                let original_len = message.content.chars().count();
                let folded_len = folded.chars().count();
                // The explanatory placeholder can be longer than a very small
                // command result. Replacing that result would grow the context,
                // and subtracting the negative saving from `usize` panics in a
                // debug build. Only commit folds that actually save space.
                if folded_len >= original_len {
                    index += 1;
                    continue;
                }
                // The text really is gone now, so the model must be allowed to ask
                // for it again. Left in the set, it would be told to scroll up to
                // a page that is no longer there.
                if let Some(key) = skill_key_of(&message.content) {
                    self.loaded.lock().unwrap().remove(&key);
                }
                total -= original_len - folded_len;
                messages[index].content = folded;
                index += 1;
            }
            if total <= budget {
                return;
            }
        }
    }
}

fn kind_tag(kind: super::types::TransferKind) -> &'static str {
    match kind {
        super::types::TransferKind::Upload => "upload",
        super::types::TransferKind::Download => "download",
    }
}

/// Written as a machine notice, and told outright not to answer it.
///
/// Every observation reaches the model in the user's turn -- that is what drives
/// the loop -- so a nudge phrased as a complaint reads as the user complaining,
/// and the model's next move is to apologise to them for something they never saw.
/// That apology is a valid "say" and lands in the thread as a reply out of
/// nowhere. The rule for anything the client says to the model about itself: name
/// the sender, say what to do, forbid the acknowledgement.
fn protocol_nudge(cut_off: bool) -> String {
    // Naming what was thrown away is what stops the resend being a different
    // reply. A model told only "that did not parse" tends to send the tail it can
    // still see -- the command -- and drop the sentence that came before it,
    // which is the one that failed.
    let opening = "The action at the end of your last reply could not be parsed as JSON, so \
                   NOTHING was carried out. Send it again, on its own and correctly escaped. ";

    /*
     * A reply that was cut off is told so, and told nothing else. Its JSON is
     * broken because it stops mid-string, not because anything was escaped
     * wrongly, and sending the escaping advice here would spend the model's one
     * retry hunting a fault that is not there.
     *
     * Otherwise the quote rule comes first, because it is what actually breaks
     * replies. A nudge that names the wrong thing is worse than a vague one: it
     * spends the model's one retry on a fix that was never needed.
     */
    let advice = if cut_off {
        "The cause is your output limit: the reply stopped mid-string and the JSON never closed. \
         Nothing was wrong with your escaping, so do not go looking for it. Send the same thing \
         again SHORTER -- change the part that differs with \"edit\" instead of sending a whole \
         file with \"write\", and split a long one across several steps."
    } else {
        "Check your DOUBLE QUOTES first: every \" inside a JSON string must be written \\\". A \
         bare \" ends the value there and everything after it becomes unparseable -- this is the \
         most common cause, especially in a command containing printf \"...\" or a quoted sed \
         script. Backslashes must be doubled too: a regex like \\1 or \\[ is written \"\\\\1\" and \
         \"\\\\[\". A newline inside a string must be written \\n, never a real line break. If you \
         were changing a file with a shell command, that is the real mistake: send it as \"edit\" \
         or \"write\" instead, where the text needs no shell quoting and cannot break this way."
    };

    format!(
        "[tshell client notice -- not from the user, who saw none of this. Do not reply to this \
         message, do not apologise, and do not promise to do better.] {opening}{advice}"
    )
}
