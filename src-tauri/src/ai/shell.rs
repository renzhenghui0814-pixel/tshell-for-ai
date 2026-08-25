//! Runs the agent's commands in the user's own terminal.
//!
//! Nothing is opened for the agent: the command is written into the interactive
//! shell exactly as if it had been typed, so the user watches it run, the working
//! directory is whatever they are standing in, and `cd`, `export` and `sudo`
//! behave the way they do when they type them themselves.
//!
//! The price of a shared terminal is that the output arrives mixed with the echo
//! of the command and the prompt. Two markers delimit the part that belongs to
//! the command. They are written as OSC escape sequences, which a terminal
//! consumes without drawing, and they are assembled by `printf` from arguments so
//! that the echo of the command line -- which carries the format string, not the
//! expanded result -- can never be mistaken for the real thing.
//!
//! What the user sees is not what was typed. Everything from the moment the line
//! is written until the opening marker comes back is the echo of the plumbing, so
//! it is held back and the plain command is drawn in its place. The terminal then
//! reads as though the command had been typed by hand, which is the whole point.
//!
//! The prompt is the part that cannot be hidden this way. A shell prints it
//! before the command is typed, so it falls outside the marker window by
//! construction, and a silent step -- the half-dozen a single file write is made
//! of -- would otherwise leave a bare prompt behind with nothing after it. A
//! silent step therefore erases the line it starts on before hiding anything,
//! taking that prompt with it; the shell prints a fresh one afterwards, so a run
//! of them leaves exactly the one prompt the user is standing at.

use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Notify;

use super::cancel::Cancel;
use super::policy::redact::{redact, strip_ansi};
use super::types::CommandResult;

/// A private OSC code. Unknown codes are consumed by the terminal, not drawn.
const OSC_CODE: &str = "97";

/// Column one, then erase the line. Two CSI sequences rather than a carriage
/// return, because this also lands in the transcript, which is read back for the
/// commands of the session: stripping the escapes leaves nothing at all, where a
/// bare CR would survive and join two prompt lines into one.
pub const ERASE_LINE: &str = "\u{1b}[G\u{1b}[2K";

const INTERRUPT: &str = "\u{3}";

pub const DEFAULT_OUTPUT_BUDGET: usize = 6000;

/// The terminal the agent borrows. Implemented over the live SSH session.
pub trait TerminalIo: Send + Sync {
    /// Writes text into the shell exactly as typing it would.
    fn write(&self, text: &str);
}

#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Hides the command and its output, for something the user did not ask for.
    pub silent: bool,
    /// Drawn in the terminal instead of what was actually typed. A file write is
    /// one intent carried out by a run of base64 chunks; the user is owed the
    /// intent.
    pub display: Option<String>,
    /// Returns the output byte for byte: no masking, no truncation. Only for
    /// output that is data rather than prose -- a file read back for editing,
    /// which is decoded here and never reaches the model. Masking a base64 blob
    /// would corrupt the file it is about to be written back as, and truncation
    /// would behead it.
    pub raw_output: bool,
}

/// Which marker the display is waiting for before it shows anything again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hide {
    Start,
    End,
}

/// How much of the session is kept for [`AgentShell::transcript`].
///
/// Stage 2 deliberately kept no transcript: a tab that is merely hidden still
/// holds its own scrollback, so there was nothing to restore. What the model
/// needs is a different thing -- the last few commands and the tail of the output
/// -- and this is the only place every chunk passes through, so it is kept here
/// and nowhere else. Bounded, because a build that scrolls for ten minutes must
/// not grow the process.
const TRANSCRIPT_BYTES: usize = 64 * 1024;

#[derive(Default)]
struct Inner {
    busy: bool,
    buffer: String,
    hide_until: Option<Hide>,
    /// Set for a silent step, cleared once the erase has actually been drawn.
    erase_wanted: bool,
    echo: String,
    /// The tail of everything the terminal has said, for `ai/context.rs`.
    transcript: String,
}

pub struct AgentShell {
    nonce: String,
    output_budget: usize,
    inner: Mutex<Inner>,
    /// Woken by [`AgentShell::observe`], so a waiting command is not polled.
    woke: Notify,
}

impl AgentShell {
    pub fn new(nonce: impl Into<String>, output_budget: usize) -> Self {
        Self {
            nonce: nonce.into(),
            output_budget,
            inner: Mutex::new(Inner::default()),
            woke: Notify::new(),
        }
    }

    /// A nonce nobody can predict from the outside is not the requirement here --
    /// what matters is that it does not collide with anything the shell prints.
    /// The clock and the process id are plenty for that.
    pub fn fresh(output_budget: usize) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.subsec_nanos())
            .unwrap_or(0);
        Self::new(
            format!("{:x}{:x}", std::process::id(), nanos),
            output_budget,
        )
    }

    #[cfg(test)]
    pub fn is_busy(&self) -> bool {
        self.inner.lock().unwrap().busy
    }

    /// The tail of the session, for the context block handed to the model.
    pub fn transcript(&self) -> String {
        self.inner.lock().unwrap().transcript.clone()
    }

    /// The cleaned output produced by the command that is currently running.
    /// A snapshot cannot lose data when a UI update arrives late.
    pub fn output_snapshot(&self) -> String {
        let inner = self.inner.lock().unwrap();
        if !inner.busy {
            return String::new();
        }
        let marker = self.start_marker();
        let Some(from) = inner.buffer.find(&marker).map(|at| at + marker.len()) else {
            return String::new();
        };
        let rest = &inner.buffer[from..];
        let raw = self.find_end(rest).map_or(rest, |(at, _, _)| &rest[..at]);
        truncate(&clean(raw, true), self.output_budget).0
    }

    fn start_marker(&self) -> String {
        format!("\u{1b}]{OSC_CODE};s{}\u{7}", self.nonce)
    }

    /// The end marker's fixed prefix. The exit code and the bell follow it.
    fn end_prefix(&self) -> String {
        format!("\u{1b}]{OSC_CODE};e{}-", self.nonce)
    }

    /// Finds the end marker in `text`, returning where it starts, where it ends,
    /// and the exit code it carried.
    fn find_end(&self, text: &str) -> Option<(usize, usize, i32)> {
        let prefix = self.end_prefix();
        let at = text.find(&prefix)?;
        let rest = &text[at + prefix.len()..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || !rest[digits.len()..].starts_with('\u{7}') {
            return None;
        }
        let end = at + prefix.len() + digits.len() + 1;
        Some((at, end, digits.parse().unwrap_or(-1)))
    }

    /// Takes everything the terminal says and returns the part the user should see.
    ///
    /// This is the only path into the buffer a running command is read from, so
    /// the pump must feed it every chunk, whether a command is running or not.
    pub fn observe(&self, chunk: &str) -> String {
        let shown = {
            let mut inner = self.inner.lock().unwrap();
            /*
             * The transcript records what was DISPLAYED, not what crossed the
             * wire, and the difference is the whole of this.
             *
             * `context.rs` reads it back as "recent commands" and "recent
             * output", and presents both to the model as the terminal's own
             * activity. What crosses the wire is the marker plumbing: the echo of
             *
             *   printf '\033]97;s%s\007' 'nonce'; eval 'pwd'; printf ...
             *
             * which the user never sees, and which `strip_ansi` cannot remove --
             * those are four literal characters `\033` in the echo of a command
             * line, not an escape sequence. So the model was reading a page of
             * plumbing per step, being told it was what had been typed. Measured
             * on a real session: twelve per cent of the whole prompt, and nine of
             * the ten "recent commands" were the wrapper rather than a command.
             *
             * Recording what was shown fixes both halves at once. A visible step
             * lands as its plain command, which is what the user watched. A
             * silent one lands as nothing, which is right for a different reason:
             * the assistant's own probes are not the terminal's activity and
             * should never have been offered to it as such.
             */
            if !inner.busy {
                Self::record(&mut inner, chunk);
                return chunk.to_string();
            }
            inner.buffer.push_str(chunk);
            match inner.hide_until {
                None => chunk.to_string(),
                Some(Hide::Start) => {
                    let marker = self.start_marker();
                    match inner.buffer.find(&marker) {
                        None => String::new(),
                        Some(at) => {
                            inner.hide_until = None;
                            // Drawn where the echo would have been, so it follows
                            // the prompt as usual.
                            let rest = inner.buffer[at + marker.len()..].to_string();
                            format!("{}\r\n{rest}", inner.echo)
                        }
                    }
                }
                Some(Hide::End) => {
                    /*
                     * The erase rides out on the first chunk of the step rather
                     * than being written when the step is started, because this is
                     * the only path to the display and the shell is what wakes it:
                     * the echo of the line always arrives, so there is always a
                     * chunk to carry it, and it lands before anything else is drawn.
                     */
                    match self.find_end(&inner.buffer) {
                        None => take_erase(&mut inner),
                        Some((_, end, _)) => {
                            inner.hide_until = None;
                            let rest = inner.buffer[end..].to_string();
                            format!("{}{rest}", take_erase(&mut inner))
                        }
                    }
                }
            }
        };
        Self::record(&mut self.inner.lock().unwrap(), &shown);
        // Outside the lock: a waiting `run` takes the same one the moment it wakes.
        self.woke.notify_waiters();
        shown
    }

    /// Keeps the tail of what the terminal has displayed, for `ai/context.rs`.
    ///
    /// Bounded, because a build that scrolls for ten minutes must not grow the
    /// process. Cut on a character boundary and only ever forward, so the tail
    /// that is kept is still decodable text.
    fn record(inner: &mut Inner, text: &str) {
        if text.is_empty() {
            return;
        }
        inner.transcript.push_str(text);
        if inner.transcript.len() > TRANSCRIPT_BYTES {
            let mut at = inner.transcript.len() - TRANSCRIPT_BYTES;
            while at < inner.transcript.len() && !inner.transcript.is_char_boundary(at) {
                at += 1;
            }
            inner.transcript.drain(..at);
        }
    }

    /// Types a command into the terminal and reads back what it produced.
    pub async fn run(
        &self,
        io: &dyn TerminalIo,
        command: &str,
        timeout_ms: u64,
        cancel: &Cancel,
        options: &RunOptions,
    ) -> Result<CommandResult, String> {
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.busy {
                return Err("The agent is already running a command in this terminal.".into());
            }
            inner.busy = true;
            inner.buffer.clear();
            inner.echo = options
                .display
                .clone()
                .unwrap_or_else(|| command.to_string());
            inner.hide_until = Some(if options.silent {
                Hide::End
            } else {
                Hide::Start
            });
            // A step nobody sees must not leave the prompt it was typed at behind.
            // A visible one keeps its prompt: that is where its command is about
            // to appear.
            inner.erase_wanted = options.silent;
        }

        io.write(&wrap_command(command, &self.nonce));

        let result = self.wait(io, timeout_ms, cancel, options).await;

        let mut inner = self.inner.lock().unwrap();
        inner.busy = false;
        // Interrupted before the opening marker, the echo stays hidden and so does
        // the command. Nothing else is worth showing: it never ran.
        inner.hide_until = None;
        // An erase that never went out is dropped rather than saved for later: it
        // would land on whatever the terminal is showing by the time it did.
        inner.erase_wanted = false;
        inner.buffer.clear();
        Ok(result)
    }

    async fn wait(
        &self,
        io: &dyn TerminalIo,
        timeout_ms: u64,
        cancel: &Cancel,
        options: &RunOptions,
    ) -> CommandResult {
        let deadline = tokio::time::sleep(Duration::from_millis(timeout_ms.max(1)));
        tokio::pin!(deadline);

        loop {
            if let Some(raw) = self.completed() {
                return self.finish(raw.0, raw.1, false, options);
            }

            // Registered before the check above is re-run, so a chunk that lands
            // between the two is not missed.
            let woken = self.woke.notified();
            if let Some(raw) = self.completed() {
                return self.finish(raw.0, raw.1, false, options);
            }

            tokio::select! {
                biased;
                () = cancel.cancelled() => return self.give_up(io, options),
                () = &mut deadline => return self.give_up(io, options),
                () = woken => continue,
            }
        }
    }

    /// The output and exit code of a command whose end marker has arrived.
    fn completed(&self) -> Option<(i32, String)> {
        let inner = self.inner.lock().unwrap();
        let marker = self.start_marker();
        let from = inner.buffer.find(&marker)? + marker.len();
        let rest = &inner.buffer[from..];
        let (at, _, code) = self.find_end(rest)?;
        Some((code, rest[..at].to_string()))
    }

    /// Ctrl+C, never a channel teardown: this is the user's terminal and it has to
    /// survive. The shell abandons the rest of the command line, so the end marker
    /// never arrives and whatever was printed so far is the result.
    fn give_up(&self, io: &dyn TerminalIo, options: &RunOptions) -> CommandResult {
        io.write(INTERRUPT);
        let raw = {
            let inner = self.inner.lock().unwrap();
            let marker = self.start_marker();
            match inner.buffer.find(&marker) {
                None => String::new(),
                Some(at) => inner.buffer[at + marker.len()..].to_string(),
            }
        };
        self.finish(-1, raw, true, options)
    }

    fn finish(
        &self,
        exit_code: i32,
        raw: String,
        timed_out: bool,
        options: &RunOptions,
    ) -> CommandResult {
        let cleaned = clean(&raw, !options.raw_output);
        let (output, truncated) = if options.raw_output {
            (cleaned, false)
        } else {
            truncate(&cleaned, self.output_budget)
        };
        CommandResult {
            output,
            exit_code,
            timed_out,
            truncated,
        }
    }
}

fn take_erase(inner: &mut Inner) -> String {
    if !inner.erase_wanted {
        return String::new();
    }
    inner.erase_wanted = false;
    ERASE_LINE.to_string()
}

/// Single-quotes a value for POSIX shells, closing and reopening around each quote.
pub fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Builds the line that is typed into the terminal. `eval` keeps the command a
/// single unit, so a trailing operator in it cannot swallow the closing marker.
pub fn wrap_command(command: &str, nonce: &str) -> String {
    format!(
        "printf '\\033]{OSC_CODE};s%s\\007' '{nonce}'; eval {}; printf '\\033]{OSC_CODE};e%s-%s\\007' '{nonce}' \"$?\"\n",
        quote(command)
    )
}

/// Turns raw terminal bytes into something worth handing to a model: the colours
/// and cursor moves a real terminal carries are noise to it, and the masking that
/// the transcript gets applies here too, because this output is sent to the same
/// place.
fn clean(raw: &str, mask: bool) -> String {
    let stripped = strip_ansi(raw);
    let text = if mask { redact(&stripped) } else { stripped };
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_start_matches('\n')
        .trim_end()
        .to_string()
}

/// Keeps the head and the tail of a long output. Both ends carry meaning: the
/// head usually says what ran, the tail usually says how it went.
pub fn truncate(text: &str, budget: usize) -> (String, bool) {
    let length = text.chars().count();
    if length <= budget {
        return (text.to_string(), false);
    }
    let head = (budget as f64 * 0.6) as usize;
    let tail = budget - head;
    let removed = length - head - tail;
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(length - tail).collect();
    (
        format!("{start}\n... [{removed} characters omitted] ...\n{end}"),
        true,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[derive(Default)]
    struct Typed {
        written: Mutex<Vec<String>>,
    }
    impl TerminalIo for Typed {
        fn write(&self, text: &str) {
            self.written.lock().unwrap().push(text.to_string());
        }
    }

    fn shell() -> AgentShell {
        AgentShell::new("n0nce", DEFAULT_OUTPUT_BUDGET)
    }

    fn start(shell: &AgentShell) -> String {
        shell.start_marker()
    }
    fn end(shell: &AgentShell, code: i32) -> String {
        format!("{}{code}\u{7}", shell.end_prefix())
    }

    /// What `context.rs` reads back has to be what the user watched, not what
    /// crossed the wire.
    ///
    /// The wire carries the marker plumbing, and the echo of it is literal text
    /// -- four characters `\033`, not an escape -- so nothing downstream can
    /// strip it. It reached the model as "recent commands", which is both a page
    /// of noise per step and a lie about what was typed.
    #[tokio::test]
    async fn the_transcript_keeps_the_command_and_not_its_plumbing() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();

        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "pwd", 5_000, &Cancel::new(), &RunOptions::default())
                    .await
            }
        });
        tokio::task::yield_now().await;

        // The shell echoes the line it was sent, plumbing and all, then answers.
        let echo = wrap_command("pwd", "n0nce");
        watcher.observe(&format!("{echo}{}", start(&shell)));
        watcher.observe("/home/trade\r\n");
        watcher.observe(&end(&shell, 0));
        running.await.unwrap().unwrap();

        let transcript = shell.transcript();
        assert!(
            transcript.contains("pwd"),
            "the command is there: {transcript:?}"
        );
        assert!(
            !transcript.contains("\\033]97"),
            "the plumbing must not be: {transcript:?}"
        );
        assert!(
            !transcript.contains("eval"),
            "nor the eval that carried it: {transcript:?}"
        );
    }

    /// A silent step is the assistant's own bookkeeping. It is not the terminal's
    /// activity and must not be offered to the model as though the user ran it.
    #[tokio::test]
    async fn a_silent_step_leaves_no_command_in_the_transcript() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();

        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                let silent = RunOptions {
                    silent: true,
                    ..Default::default()
                };
                shell
                    .run(&io, "uname -r", 5_000, &Cancel::new(), &silent)
                    .await
            }
        });
        tokio::task::yield_now().await;

        let echo = wrap_command("uname -r", "n0nce");
        watcher.observe(&format!("{echo}{}", start(&shell)));
        watcher.observe("5.14.0\r\n");
        watcher.observe(&end(&shell, 0));
        running.await.unwrap().unwrap();

        let transcript = shell.transcript();
        assert!(
            !transcript.contains("uname"),
            "a silent step is not activity: {transcript:?}"
        );
        assert!(
            !transcript.contains("\\033]97"),
            "and neither is its plumbing"
        );
    }

    /// What the user types is the main thing this exists to record, and none of
    /// the above may touch it.
    #[test]
    fn what_the_user_types_is_recorded_as_it_arrives() {
        let shell = shell();
        shell.observe("[me@box ~]$ ls -la\r\ntotal 8\r\n");
        assert!(shell.transcript().contains("[me@box ~]$ ls -la"));
    }

    #[tokio::test]
    async fn running_output_can_be_read_before_the_command_finishes() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();
        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "slow", 5_000, &Cancel::new(), &RunOptions::default())
                    .await
            }
        });
        tokio::task::yield_now().await;
        watcher.observe(&format!("{}first line\r\n", start(&watcher)));
        assert_eq!(watcher.output_snapshot(), "first line");
        watcher.observe(&end(&watcher, 0));
        running.await.unwrap().unwrap();
    }

    #[test]
    fn the_typed_line_carries_both_markers_and_the_command() {
        let line = wrap_command("ls -la", "abc");
        assert!(line.starts_with("printf '\\033]97;s%s\\007' 'abc'; eval 'ls -la'; "));
        assert!(line.ends_with("'abc' \"$?\"\n"));
    }

    #[test]
    fn a_quote_in_the_command_survives_being_typed() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        let line = wrap_command("echo 'hi'", "abc");
        assert!(line.contains(r"eval 'echo '\''hi'\'''"));
    }

    #[tokio::test]
    async fn a_visible_command_draws_the_command_and_then_its_output() {
        let shell = Arc::new(shell());
        let io = Typed::default();
        let watcher = shell.clone();

        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "ls", 5_000, &Cancel::new(), &RunOptions::default())
                    .await
            }
        });
        // Let the command be typed before anything is fed back.
        tokio::task::yield_now().await;

        // The echo of the plumbing is held back until the opening marker.
        assert_eq!(watcher.observe("printf '\\033]97;s%s"), "");
        let shown = watcher.observe(&format!("{}total 0\r\n", start(&watcher)));
        assert_eq!(shown, "ls\r\ntotal 0\r\n");
        watcher.observe(&end(&watcher, 0));

        let result = running.await.unwrap().unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.output, "total 0");
        assert!(!result.timed_out);
        drop(io);
    }

    #[tokio::test]
    async fn a_silent_step_erases_its_prompt_and_shows_nothing_else() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();
        let options = RunOptions {
            silent: true,
            ..Default::default()
        };

        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "cat /tmp/x", 5_000, &Cancel::new(), &options)
                    .await
            }
        });
        tokio::task::yield_now().await;

        // The erase rides out on the first chunk, and nothing after it is drawn.
        assert_eq!(watcher.observe("echo of the line\r\n"), ERASE_LINE);
        assert_eq!(watcher.observe(&format!("{}payload", start(&watcher))), "");
        assert_eq!(watcher.observe(&end(&watcher, 0)), "");

        let result = running.await.unwrap().unwrap();
        assert_eq!(result.output, "payload");
    }

    #[tokio::test]
    async fn the_exit_code_comes_off_the_closing_marker() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();
        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "false", 5_000, &Cancel::new(), &RunOptions::default())
                    .await
            }
        });
        tokio::task::yield_now().await;
        watcher.observe(&format!("{}{}", start(&watcher), end(&watcher, 137)));

        let result = running.await.unwrap().unwrap();
        assert_eq!(result.exit_code, 137);
    }

    #[tokio::test(start_paused = true)]
    async fn a_command_that_never_returns_is_interrupted_rather_than_torn_down() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();
        let io = Arc::new(Typed::default());

        let running = tokio::spawn({
            let shell = shell.clone();
            let io = io.clone();
            async move {
                shell
                    .run(
                        io.as_ref(),
                        "sleep 999",
                        30_000,
                        &Cancel::new(),
                        &RunOptions::default(),
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;
        watcher.observe(&format!("{}partial output", start(&watcher)));
        tokio::time::advance(Duration::from_millis(31_000)).await;

        let result = running.await.unwrap().unwrap();
        assert!(result.timed_out);
        assert_eq!(result.exit_code, -1);
        assert_eq!(result.output, "partial output");
        assert!(io
            .written
            .lock()
            .unwrap()
            .iter()
            .any(|line| line == INTERRUPT));
    }

    #[tokio::test]
    async fn stopping_interrupts_the_same_way_a_timeout_does() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();
        let cancel = Cancel::new();
        let io = Arc::new(Typed::default());

        let running = tokio::spawn({
            let shell = shell.clone();
            let cancel = cancel.clone();
            let io = io.clone();
            async move {
                shell
                    .run(
                        io.as_ref(),
                        "sleep 999",
                        60_000,
                        &cancel,
                        &RunOptions::default(),
                    )
                    .await
            }
        });
        tokio::task::yield_now().await;
        watcher.observe(&start(&watcher));
        cancel.cancel();

        let result = running.await.unwrap().unwrap();
        assert!(result.timed_out);
        assert!(io
            .written
            .lock()
            .unwrap()
            .iter()
            .any(|line| line == INTERRUPT));
    }

    #[tokio::test]
    async fn output_is_cleaned_of_escapes_and_masked_unless_it_is_data() {
        let shell = Arc::new(shell());
        let watcher = shell.clone();
        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "env", 5_000, &Cancel::new(), &RunOptions::default())
                    .await
            }
        });
        tokio::task::yield_now().await;
        watcher.observe(&format!(
            "{}\x1b[2mPASSWORD=hunter2\x1b[0m\r\n{}",
            start(&watcher),
            end(&watcher, 0)
        ));

        let result = running.await.unwrap().unwrap();
        assert_eq!(result.output, "PASSWORD=[REDACTED]");
    }

    #[tokio::test]
    async fn raw_output_is_handed_back_byte_for_byte() {
        let shell = Arc::new(AgentShell::new("n0nce", 10));
        let watcher = shell.clone();
        let options = RunOptions {
            raw_output: true,
            ..Default::default()
        };
        let running = tokio::spawn({
            let shell = shell.clone();
            async move {
                let io = Typed::default();
                shell
                    .run(&io, "base64 x", 5_000, &Cancel::new(), &options)
                    .await
            }
        });
        tokio::task::yield_now().await;
        let payload = "PASSWORD=hunter2 and a very long base64 blob";
        watcher.observe(&format!("{}{payload}{}", start(&watcher), end(&watcher, 0)));

        let result = running.await.unwrap().unwrap();
        assert_eq!(result.output, payload, "neither masked nor truncated");
        assert!(!result.truncated);
    }

    #[test]
    fn a_long_output_keeps_both_ends() {
        let text = "x".repeat(100);
        let (out, truncated) = truncate(&text, 20);
        assert!(truncated);
        assert!(out.starts_with(&"x".repeat(12)));
        assert!(out.contains("[80 characters omitted]"));

        let (out, truncated) = truncate("short", 20);
        assert_eq!(out, "short");
        assert!(!truncated);
    }

    #[test]
    fn nothing_is_hidden_while_no_command_is_running() {
        let shell = shell();
        assert!(!shell.is_busy());
        assert_eq!(
            shell.observe("an ordinary prompt$ "),
            "an ordinary prompt$ "
        );
    }

    #[test]
    fn a_half_written_end_marker_is_not_an_end_marker() {
        let shell = shell();
        let partial = format!("{}12", shell.end_prefix());
        assert!(shell.find_end(&partial).is_none());
        let whole = format!("{}12\u{7}", shell.end_prefix());
        assert_eq!(shell.find_end(&whole).map(|found| found.2), Some(12));
    }
}
