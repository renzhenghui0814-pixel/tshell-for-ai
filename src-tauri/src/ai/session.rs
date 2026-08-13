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
use super::llm::say::SayStreamer;
use super::llm::{ChatMessage, CompletionRequest, LlmError, LlmFailure, TokenUsage, TransportInfo, Watcher};
use super::moves::TransferRequest;
use super::parse::{is_prose, parse_actions};
use super::policy::command::Verdict;
use super::policy::path::PathVerdict;
use super::prompt::{
    describe_memory, describe_result, describe_skill_failure, estimate_tokens, fold_output,
    fold_skill, skill_header, skill_key_of, KEEP_RECENT, MAX_FACT_CHARS,
};
use super::store::memory::{AppendOutcome, RemoveOutcome};
use super::store::skill::MAIN_FILE;
use super::types::{
    ActionKind, AgentAction, AgentEvent, AgentMode, MemoryOpKind, MemoryOutcome, MemoryScope,
    SkillOutcome, Unconfirmed,
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

impl<H: AgentHost + 'static> Watcher for StepWatcher<H> {
    fn delta(&self, text: &str) {
        self.host.emit(AgentEvent::Delta { text: text.to_string() });
    }
    fn reasoning(&self, text: &str) {
        self.host.emit(AgentEvent::Reasoning { text: text.to_string() });
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
        self.host.emit(AgentEvent::Retry { attempt, kind: kind.tag().to_string() });
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
        !is_prose(text)
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
                            item["content"].as_str().unwrap_or_default().to_string(),
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
    fn logged_response(&self, text: &str) {
        self.log.response(text);
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
    /// A notice waiting to ride out on the next observation.
    ///
    /// Only ever set for a reply that carried more than one action. The loop runs
    /// one action per step and silently threw the rest away, which the model had
    /// no way of knowing: it was told the first one's result and read that as the
    /// result of everything it sent, so an edit it believed had landed had not.
    dropped: Mutex<String>,
    cancel: Mutex<Option<Cancel>>,
    running: AtomicBool,
    log: Arc<super::store::log::LogSession>,
}

impl<H: AgentHost + 'static> AgentSession<H> {
    pub fn new(host: Arc<H>, config: AgentConfig) -> Self {
        Self {
            host,
            config: Mutex::new(config),
            messages: Mutex::new(Vec::new()),
            loaded: Mutex::new(HashSet::new()),
            dropped: Mutex::new(String::new()),
            cancel: Mutex::new(None),
            running: AtomicBool::new(false),
            log: Arc::new(super::store::log::LogSession::inactive()),
        }
    }

    pub fn with_log(mut self, log: Arc<super::store::log::LogSession>) -> Self {
        self.log = log;
        self
    }

    /// How the model should answer "what are you?", taken from whichever endpoint
    /// is currently wired in.
    pub fn describe_provider(&self) -> String {
        self.config.lock().unwrap().provider.describe()
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
        self.messages.lock().unwrap().iter().map(|message| message.content.chars().count()).sum()
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
    pub fn restore(&self, messages: Vec<ChatMessage>) {
        self.reset();
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

    pub async fn send(&self, user_text: &str) {
        if self.is_running() {
            return;
        }
        self.messages.lock().unwrap().push(ChatMessage::user(user_text));
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
        /*
         * The notice belongs to the task that raised it and dies with it.
         *
         * It is only ever consumed by the next observation, and a task that ended
         * on "say", "ask" or "done" made no further observation -- so the notice
         * sat here until some later task, minutes or days away, made one. It then
         * arrived attached to an unrelated command's output, telling the model
         * that actions it had never sent were discarded.
         */
        self.dropped.lock().unwrap().clear();
        *self.cancel.lock().unwrap() = None;
        self.host.emit(AgentEvent::Idle);
    }

    async fn steps(&self, cancel: &Cancel) -> Result<(), LlmError> {
        let mut repairs = 0;
        let mut blanks = 0;
        /*
         * Whether this task has actually carried something out yet.
         *
         * What separates "the model answered a question" from "the model narrated
         * a step and forgot to send it". Before the first action either reading is
         * possible and the generous one is right; after it, a reply with no action
         * in it has left the task hanging. Deliberately not `step > 0`, which also
         * counts the rounds spent nudging a model back to the protocol -- those
         * carried nothing out and must not make prose look like a lost step.
         */
        let mut acted = false;
        /*
         * One narration is worth asking about. Two IN A ROW means the model meant
         * it. In a row, like `repairs`, and for the same reason: two accidents
         * twenty steps apart are two accidents.
         */
        let mut prose_nudges = 0;

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
            self.host.emit(AgentEvent::Context { chars: self.context_chars() as u32 });

            let watcher = Arc::new(StepWatcher {
                host: self.host.clone(),
                billed: AtomicBool::new(false),
                cut_off: AtomicBool::new(false),
                log: self.log.clone(),
            });
            let (system, timeout_ms) = {
                let config = self.config.lock().unwrap();
                (config.system.clone(), config.request_timeout_ms)
            };
            let request = CompletionRequest {
                system: system.clone(),
                messages: self.messages.lock().unwrap().clone(),
                timeout_ms,
                cancel: cancel.clone(),
                watcher: watcher.clone(),
            };

            // The handle is taken and the lock let go before the request starts:
            // it lasts as long as the model takes to answer, and `configure` must
            // not be blocked behind it.
            let provider = self.config.lock().unwrap().provider.clone();
            let reply = provider.complete(request).await;

            let reply = match reply {
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
                        self.host
                            .emit(AgentEvent::Retry { attempt: blanks, kind: "empty".into() });
                        continue;
                    }
                    return Err(error);
                }
            };
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
                    completion: estimate_tokens(&reply),
                    estimated: Some(true),
                });
            }
            if cancel.is_cancelled() {
                self.host.emit(AgentEvent::Stopped);
                return Ok(());
            }
            self.messages.lock().unwrap().push(ChatMessage::assistant(&reply));

            // The reply as it was written is already in the log, put there by the
            // provider that received it. What this client made of it is visible in
            // the thread, and in what the next request carries.
            let parsed = parse_actions(&reply);
            /*
             * A reply whose FIRST action would not parse runs nothing at all.
             *
             * Running the next one instead is what this used to do, and it was the
             * worst of the three possible outcomes. The model meant "say this,
             * then run that"; the say was lost to a real line break inside its
             * string, so the command ran, the sentence never appeared, and the
             * model carried on believing the user had read it.
             */
            let action = if parsed.leading { None } else { parsed.actions.first().cloned() };
            /*
             * One step is one action, and a reply that carried more has just had
             * the rest dropped. Saying so is the whole fix: the model was told the
             * first one's result and read it as the result of everything it sent.
             */
            let total = parsed.actions.len() + parsed.unreadable as usize;
            if action.is_some() && total > 1 {
                let unparsed = if parsed.unreadable > 0 {
                    format!(", {} of which could not be parsed as JSON", parsed.unreadable)
                } else {
                    String::new()
                };
                *self.dropped.lock().unwrap() = format!(
                    "[tshell client notice -- not from the user. Do not reply to this message, do \
                     not apologise.] Your last reply carried {total} actions{unparsed}. One step \
                     is ONE action: only the first was carried out and the rest were discarded. \
                     The result below is that first action alone. Send the next one on its own, \
                     and do not assume any of the others happened."
                );
            }

            let Some(action) = action else {
                match self.handle_no_action(&reply, acted, cut_off, &mut prose_nudges) {
                    NoAction::Done => return Ok(()),
                    NoAction::Continue => {
                        step += 1;
                        continue;
                    }
                    NoAction::Nudge => {}
                }
                repairs += 1;
                if repairs > 1 {
                    self.host.emit(AgentEvent::Error {
                        message: "The model did not reply with a single JSON object, twice in a row."
                            .into(),
                        code: Some("protocol".into()),
                    });
                    return Ok(());
                }
                self.observe(&protocol_nudge(parsed.leading, parsed.actions.len(), cut_off));
                step += 1;
                continue;
            };

            // Both counters are about a run of bad replies, and this is the line
            // that says the run is over: something parsed and is about to be
            // carried out.
            repairs = 0;
            prose_nudges = 0;

            let kind = action.action.unwrap_or(ActionKind::Say);
            match kind {
                ActionKind::Say => {
                    let text = action
                        .text
                        .clone()
                        .or_else(|| action.summary.clone())
                        .unwrap_or_default();
                    self.host.emit(AgentEvent::Reply { text });
                    return Ok(());
                }
                ActionKind::Ask => {
                    self.host.emit(AgentEvent::Question {
                        question: action.question.clone().unwrap_or_default(),
                    });
                    return Ok(());
                }
                ActionKind::Done => {
                    self.host.emit(AgentEvent::Summary {
                        summary: action.summary.clone().unwrap_or_default(),
                    });
                    return Ok(());
                }
                _ => {}
            }

            /*
             * Past here the step carries something out, which is what makes a
             * later reply with no action in it a lost step rather than an answer.
             * Set before the work rather than after it: an action that fails, is
             * refused or is declined has still been taken.
             */
            acted = true;
            /*
             * What the model said on its way to acting, shown before the card it
             * drew. Deliberately below the three spoken verbs rather than above
             * them: for those the text field IS the answer and it is already being
             * streamed into a bubble.
             */
            if !parsed.preamble.is_empty() {
                self.host.emit(AgentEvent::Reply { text: parsed.preamble.clone() });
            }

            if kind.as_file_op().is_some() {
                self.run_file_action(&action, cancel).await;
            } else if kind.as_memory_op().is_some() {
                self.run_memory_action(&action).await;
            } else if kind == ActionKind::Skill {
                self.run_skill_action(&action).await;
            } else if kind.as_transfer().is_some() {
                self.run_transfer_action(&action, cancel).await;
            } else {
                self.run_command_action(&action, cancel).await;
            }
            step += 1;
        }

        self.host.emit(AgentEvent::StepLimit { steps: max_steps });
        Ok(())
    }

    /// What to do with a reply that carried no action this step could run.
    fn handle_no_action(
        &self,
        reply: &str,
        acted: bool,
        cut_off: bool,
        prose_nudges: &mut u32,
    ) -> NoAction {
        /*
         * Prose is an answer -- until the task has started, after which it is a
         * step that lost its action.
         *
         * Before anything has been carried out, a reply with nothing action-shaped
         * in it was never an attempt at the protocol: it is the model talking,
         * because the question it was asked wanted an answer rather than a
         * command. Once an action HAS run, the same shape means the model
         * announced its next move and never sent it.
         */
        if is_prose(reply) {
            self.host.emit(AgentEvent::Reply { text: reply.trim().to_string() });
            if acted && *prose_nudges == 0 {
                *prose_nudges += 1;
                self.observe(
                    "[tshell client notice -- not from the user, who saw none of this. Do not \
                     reply to this message and do not apologise.] That reply described what you \
                     were about to do but did not carry the action that does it, so NOTHING \
                     happened and the task is still waiting on you. Send that action now, as \
                     exactly one JSON object and nothing else. If the work is genuinely finished, \
                     send {\"action\":\"done\",\"summary\":\"...\"}. If you need a decision only \
                     the user can make, send {\"action\":\"ask\",\"question\":\"...\"}.",
                );
                return NoAction::Continue;
            }
            /*
             * A reply that ran out of room is shown and then said to be short. Not
             * retried: the ceiling that stopped it is still there. Not hidden
             * either -- the text stops mid-sentence, and a reader who is not told
             * why is left thinking the assistant froze.
             */
            if cut_off {
                self.emit_truncated();
            }
            return NoAction::Done;
        }

        /*
         * An answer whose object never closed is kept, not thrown away.
         *
         * Far more often than running out of room, the model simply finishes its
         * sentence and forgets the closing `"}`. Salvaged through the streamer for
         * two reasons: it decodes the open string exactly as the panel already
         * painted it, and it yields nothing at all for a verb not addressed to the
         * user -- so an unterminated `write` or `run` falls through to the nudge.
         * Half a file's content must never become a whole instruction.
         */
        let mut streamer = SayStreamer::new();
        let partial = streamer.push(reply).trim().to_string();
        /*
         * Only when the string was still open at the end of the reply. Without
         * that condition this also fires on a bare `"` inside the text, which
         * closes the string early and leaves the rest of the reply after it --
         * handing the user the handful of words before the stray quote as though
         * they were the answer.
         */
        if !partial.is_empty() && streamer.unterminated() {
            self.host.emit(AgentEvent::Reply { text: partial });
            // Only when the room really did run out. The far commoner case is a
            // finished sentence missing its brace, and telling the reader that
            // stops mid-sentence -- when it plainly does not -- would teach them
            // to distrust the notice on the occasions it is true.
            if cut_off {
                self.emit_truncated();
            }
            return NoAction::Done;
        }

        NoAction::Nudge
    }

    fn emit_truncated(&self) {
        self.host.emit(AgentEvent::Error {
            message: "The model ran out of room and the reply above stops mid-sentence.".into(),
            code: Some("truncatedReply".into()),
        });
    }

    async fn run_command_action(&self, action: &AgentAction, cancel: &Cancel) {
        let command = action.command.clone().unwrap_or_default().trim().to_string();
        if command.is_empty() {
            self.observe("The run action needs a non-empty command.");
            return;
        }

        let policy = self.host.classify(&command);
        let why = action.why.clone().unwrap_or_default();

        if policy.verdict == Verdict::Refuse {
            let reasons = self.host.describe_risk(&policy.reasons);
            self.host.emit(AgentEvent::Refused { command: command.clone(), reasons });
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
        let kind = action.action.and_then(ActionKind::as_file_op).expect("a file verb");
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
            self.host
                .emit(AgentEvent::Refused { command: format!("{} {path}", kind.tag()), reasons });
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
        let kind = action.action.and_then(ActionKind::as_transfer).expect("a transfer verb");
        let paths: Vec<String> =
            action.paths.iter().filter(|path| !path.is_empty()).cloned().collect();
        let to = action.to.clone().unwrap_or_default().trim().to_string();
        let why = action.why.clone().unwrap_or_default();

        if !to.is_empty() && !self.refuse_target(kind, &to) {
            return;
        }

        let arrow = if to.is_empty() { String::new() } else { format!("-> {to}") };
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
                self.observe(&format!("The {} did not happen: {}", kind_tag(kind), error.0));
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

    /// One step of the loop, for the two actions that change what is remembered.
    ///
    /// The shortest step there is: no machine, no plan, no confirmation. It is
    /// worth being a step at all rather than something inferred afterwards because
    /// the model decides what is worth keeping while it still has the reason in
    /// front of it, and because the result has to come back -- a full scope or a
    /// line that matched nothing is something it must act on.
    async fn run_memory_action(&self, action: &AgentAction) {
        let op = action.action.and_then(ActionKind::as_memory_op).expect("a memory verb");
        let scope = action.scope.unwrap_or(MemoryScope::Server);
        let text = action.text.clone().unwrap_or_default();

        if !self.host.memory_enabled() {
            self.observe(
                "Memory is switched off for this setup, so nothing was stored. Carry on without \
                 it, and do not try again.",
            );
            return;
        }
        if text.trim().is_empty() {
            self.observe(&format!(
                "The {} action needs a non-empty \"text\". Send it again with the fact on one line.",
                match op {
                    MemoryOpKind::Remember => "remember",
                    MemoryOpKind::Forget => "forget",
                }
            ));
            return;
        }
        /*
         * A ceiling on one line, because the prompt's rule about it has already
         * been shown not to hold on its own. Length is the one signal for a
         * transcribed paragraph that cannot misread a genuine fact, and refusing
         * is safe either way, because the answer sends the model back to write a
         * shorter one rather than stopping the task.
         *
         * Only `remember` is measured. `forget` names a line that is already in
         * the file, so a long one there is the user's own writing being matched.
         */
        let length = text.chars().count();
        if op == MemoryOpKind::Remember && length > MAX_FACT_CHARS {
            self.host.emit(AgentEvent::Memory {
                op,
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
                    "{MAX_FACT_CHARS}. A line that long is a piece of command output written out, \
                     not a fact"
                ),
                "about how this setup is put together. Either boil it down to one short sentence \
                 that"
                    .into(),
                "would still be true if you had run nothing today, or drop it and put it in your \
                 answer"
                    .into(),
                "to the user instead. Do not send this line again. This does not block the task."
                    .into(),
            ]
            .join("\n"));
            return;
        }

        let (outcome, token) = match op {
            MemoryOpKind::Remember => {
                let result = self.host.remember(scope, &text).await;
                let outcome = match result.outcome {
                    AppendOutcome::Ok => MemoryOutcome::Ok,
                    AppendOutcome::Duplicate => MemoryOutcome::Duplicate,
                    AppendOutcome::Full => MemoryOutcome::Full,
                    AppendOutcome::Failed => MemoryOutcome::Failed,
                };
                (outcome, result.token)
            }
            MemoryOpKind::Forget => {
                let outcome = match self.host.forget(scope, &text).await {
                    RemoveOutcome::Ok => MemoryOutcome::Ok,
                    RemoveOutcome::Missing => MemoryOutcome::Missing,
                    RemoveOutcome::Ambiguous => MemoryOutcome::Ambiguous,
                    RemoveOutcome::Failed => MemoryOutcome::Failed,
                };
                (outcome, None)
            }
        };
        self.host.emit(AgentEvent::Memory { op, scope, text, outcome, token });
        self.observe(&describe_memory(op, scope, outcome));
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

        let Some(content) = result.content.filter(|_| result.outcome == SkillOutcome::Ok) else {
            self.observe(&describe_skill_failure(result.outcome, &id, &read_file));
            return;
        };

        self.loaded.lock().unwrap().insert(format!("{id}::{read_file}"));
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
    ///
    /// A dropped-actions notice rides on the front of the next one rather than
    /// going in as a turn of its own, so the model never receives two user
    /// messages in a row -- and so the warning arrives attached to the result it
    /// qualifies.
    fn observe(&self, text: &str) {
        let notice = std::mem::take(&mut *self.dropped.lock().unwrap());
        let content =
            if notice.is_empty() { text.to_string() } else { format!("{notice}\n\n{text}") };
        self.messages.lock().unwrap().push(ChatMessage::user(content));
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
        let mut total: usize = messages.iter().map(|message| message.content.chars().count()).sum();
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
                if message.role != super::llm::ChatRole::User {
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
                // The text really is gone now, so the model must be allowed to ask
                // for it again. Left in the set, it would be told to scroll up to
                // a page that is no longer there.
                if let Some(key) = skill_key_of(&message.content) {
                    self.loaded.lock().unwrap().remove(&key);
                }
                total -= message.content.chars().count() - folded.chars().count();
                messages[index].content = folded;
                index += 1;
            }
            if total <= budget {
                return;
            }
        }
    }
}

enum NoAction {
    /// The task is over; the reply was the answer.
    Done,
    /// A nudge has already been queued; go round again.
    Continue,
    /// Nothing usable at all; the protocol nudge below applies.
    Nudge,
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
fn protocol_nudge(leading: bool, followed: usize, cut_off: bool) -> String {
    let opening = if leading {
        // Naming what was thrown away is what stops the resend being a different
        // reply. A model told only "that did not parse" tends to send the tail it
        // can still see -- the command -- and drop the sentence that came before
        // it, which is the one that failed.
        let tail = if followed > 0 {
            format!(" -- not it, and not the {followed} that followed it")
        } else {
            String::new()
        };
        format!(
            "The FIRST action in your last reply could not be parsed as JSON, so NOTHING was \
             carried out{tail}. Send that action again, on its own and correctly escaped. "
        )
    } else {
        "Your last reply could not be parsed as a JSON object. Send the SAME thing again as \
         exactly one JSON object and nothing else -- if it was an answer for the user, it goes in \
         {\"action\":\"say\",\"text\":\"...\"}. "
            .to_string()
    };

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
