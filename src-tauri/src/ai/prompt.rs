//! What the model is told, and what it is told back.
//!
//! Every function here is a string in and a string out. That is what makes them
//! the part of the assistant worth pinning down hardest: a reply that parses
//! wrongly is a bug report, but guidance that drifts changes what the model does
//! on someone's machine, quietly, and nothing on screen says so.

use crate::config::Language;

use super::types::{CommandResult, MemoryOutcome, MemoryScope, SkillOutcome};

/// The longest one remembered fact may be.
///
/// Set where a written-out sentence in either language still fits easily and a
/// pasted paragraph does not. Chinese packs more into a character than English,
/// so this is generous rather than tight: the point is to catch output that has
/// been transcribed, not to police how a fact is worded.
pub const MAX_FACT_CHARS: usize = 240;

/// Roughly three steps: an action and its observation apiece.
pub const KEEP_RECENT: usize = 6;

/// Han, kana and hangul, which tokenise at roughly one character each where most
/// other scripts run nearer four characters to the token.
fn is_wide(char: char) -> bool {
    matches!(char as u32,
        0x1100..=0x11FF
        | 0x2E80..=0x9FFF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7FF
        | 0xF900..=0xFAFF
        | 0xFE30..=0xFE4F
        | 0xFF00..=0xFFEF)
}

/// What a piece of text will have cost, near enough to put on screen.
///
/// Deliberately arithmetic and not a tokeniser. Carrying a real one would mean
/// carrying the vocabulary of whichever model the user pointed at, which is a
/// megabyte of data to turn a number that is shown with a tilde on it into a
/// number that is shown with a tilde on it.
pub fn estimate_tokens(text: &str) -> u32 {
    let wide = text.chars().filter(|c| is_wide(*c)).count();
    // UTF-16 units, because the arithmetic this reproduces was written against
    // JavaScript's `String.length` and a surrogate pair counted two there.
    let length = text.encode_utf16().count();
    let narrow = length.saturating_sub(wide);
    (wide as f64 + narrow as f64 / 4.0).ceil() as u32
}

/// The line a loaded skill sits under.
///
/// It is what makes a skill body findable afterwards: the context trimmer folds
/// by it, and a reopened conversation counts what is already loaded by it.
/// Deliberately not shaped like `OUTPUT:` -- the two are folded in separate
/// passes and must never match each other's pattern.
pub fn skill_header(id: &str, file: &str) -> String {
    format!("SKILL {id} ({file}):")
}

/// The header a command's output is written under, in both its forms. Matched at
/// the start of a line, which is what `(?m)^...$` meant in the original.
fn find_output_header(content: &str) -> Option<usize> {
    let mut at = 0;
    for line in content.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if text == "OUTPUT:" || text == "OUTPUT (middle omitted):" {
            return Some(at);
        }
        at += line.len();
    }
    None
}

/// An observation with its output replaced by a note of what was there, or
/// `None` when there is nothing in it worth dropping.
///
/// Everything after the header goes, which takes the failure guidance at the end
/// with it. That guidance tells the model to diagnose and carry on, and it is
/// addressed to a step that has long since been diagnosed and carried on from.
pub fn fold_output(content: &str) -> Option<String> {
    let at = find_output_header(content)?;
    let head = content[..at].trim_end();
    let dropped = content.len() - head.len();
    Some(format!(
        "{head}\n[Output omitted here: {dropped} characters, dropped to stay inside the context \
         budget. You have already read it and acted on it. Run the command again if you need it back.]"
    ))
}

/// The `id` and `file` of a skill header line, wherever it sits.
fn skill_header_parts(content: &str) -> Option<(usize, String, String)> {
    let mut at = 0;
    for line in content.split_inclusive('\n') {
        let text = line.trim_end_matches(['\n', '\r']);
        if let Some(rest) = text.strip_prefix("SKILL ") {
            if let Some(open) = rest.find(" (") {
                let id = &rest[..open];
                let tail = &rest[open + 2..];
                if let Some(close) = tail.rfind("):") {
                    let file = &tail[..close];
                    let idish = !id.is_empty()
                        && id
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
                    if idish && !file.is_empty() && close + 2 == tail.len() {
                        return Some((at, id.to_string(), file.to_string()));
                    }
                }
            }
        }
        at += line.len();
    }
    None
}

/// The `id::file` a skill body belongs to, or `None` for anything else.
pub fn skill_key_of(content: &str) -> Option<String> {
    skill_header_parts(content).map(|(_, id, file)| format!("{id}::{file}"))
}

/// A loaded skill with its body replaced by a note of what was there.
///
/// The header goes too, unlike a folded command where the command itself stays.
/// A command is worth keeping as a record of what was tried; a skill that is no
/// longer in the context is not a step in the task, and leaving its header would
/// only invite the model to reason about a document it can no longer read.
pub fn fold_skill(content: &str) -> Option<String> {
    let (at, id, file) = skill_header_parts(content)?;
    if at != 0 {
        return None;
    }
    Some(format!(
        "[The skill {id} ({file}) was loaded here and its {} characters have been dropped to stay \
         inside the context budget. You have already read it and acted on it. Load it again if you \
         need it back.]",
        content.len()
    ))
}

/// Shells report a fatal signal as 128 + n, which is worth spelling out.
fn signal_name(number: i32) -> Option<&'static str> {
    Some(match number {
        1 => "SIGHUP",
        2 => "SIGINT",
        3 => "SIGQUIT",
        6 => "SIGABRT",
        8 => "SIGFPE",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        13 => "SIGPIPE",
        15 => "SIGTERM",
        _ => return None,
    })
}

pub fn describe_exit(code: i32) -> String {
    let signal = if code > 128 && code < 192 {
        signal_name(code - 128)
    } else {
        None
    };
    match signal {
        Some(signal) => format!("EXIT: {code} (killed by {signal})"),
        None => format!("EXIT: {code}"),
    }
}

pub fn describe_result(result: &CommandResult) -> String {
    if result.timed_out {
        return [
            "TIMED OUT. Ctrl+C was sent to the terminal, so the command was interrupted and",
            "the exit code is unknown. The shell itself is intact: the working directory and",
            "anything exported earlier are still there. Partial output follows.",
            "OUTPUT:",
            if result.output.is_empty() {
                "(nothing)"
            } else {
                &result.output
            },
            "",
            "Decide whether the command needed to run that long. If it did, run it in the",
            "background or narrow it down; otherwise work out what it was waiting for.",
        ]
        .join("\n");
    }

    let mut lines = vec![
        describe_exit(result.exit_code),
        if result.truncated {
            "OUTPUT (middle omitted):".into()
        } else {
            "OUTPUT:".into()
        },
        if result.output.is_empty() {
            "(no output)".into()
        } else {
            result.output.clone()
        },
    ];
    if result.exit_code != 0 {
        // Left as the last thing the model reads, because that is where it looks first.
        lines.extend([
            String::new(),
            "This step failed. That is part of the task, not the end of it: work out the cause"
                .into(),
            "from the output above and carry on. Read the relevant log or config file, check"
                .into(),
            "whether the port, file or permission it needs is actually available, then fix it"
                .into(),
            "and retry. Do not stop and report this failure as though it were the answer.".into(),
        ]);
    }
    lines.join("\n")
}

/// What a memory write is told about itself.
///
/// Every unhappy answer ends by pointing at the next move and, where the model
/// might read a refusal as a wall, says outright that the task continues. A
/// failed write is the one worth spelling out: unmentioned, the model would go on
/// believing it had recorded something that is not there.
pub fn describe_memory(scope: MemoryScope, outcome: MemoryOutcome) -> String {
    let (where_, where_capital) = match scope {
        MemoryScope::Global => ("global memory", "Global memory"),
        MemoryScope::Server => ("this server's memory", "This server's memory"),
    };
    match outcome {
        MemoryOutcome::Ok => format!("Stored in {where_}. Carry on with what you were doing."),
        MemoryOutcome::Duplicate => format!(
            "{where_capital} already says that, so nothing was written. Do not record it again; carry on."
        ),
        MemoryOutcome::Full => [
            format!("{where_capital} is full and NOTHING was written."),
            "There is no action for making room -- the user prunes their own notes. Put what you".into(),
            "wanted to record in your answer instead. This does not block the task.".into(),
        ]
        .join("
"),
        MemoryOutcome::Off => "Memory is switched off for this setup. Carry on without it.".to_string(),
        _ => [
            format!("{where_capital} could not be written and the line was NOT saved."),
            "Do not rely on it being there later. This does not block the task; carry on.".into(),
        ]
        .join("
"),
    }
}

/// What a skill load that produced nothing is told about itself.
///
/// Every answer names the next move, because a model that has just been refused
/// a document tends to either try the same name again or abandon the task, and
/// both are worse than getting on with it unaided. A skill is an aid, never a
/// dependency: no failure here blocks anything.
pub fn describe_skill_failure(outcome: SkillOutcome, id: &str, file: &str) -> String {
    let notice =
        "[tshell client notice -- not from the user, who saw none of this. Do not reply to \
                  this message and do not apologise.] ";
    match outcome {
        SkillOutcome::Unknown => format!(
            "{notice}There is no skill called \"{id}\". Only the skills listed for you exist, and \
             the name must be copied exactly. Carry on with the task using what you know."
        ),
        SkillOutcome::Disabled => format!(
            "{notice}The skill \"{id}\" has been switched off by the user and cannot be read. Do \
             not ask them to turn it on. Carry on with the task using what you know."
        ),
        SkillOutcome::Missing => format!(
            "{notice}The skill \"{id}\" has no file at \"{file}\". Load its main document with \
             {{\"action\":\"skill\",\"name\":\"{id}\"}} and take the paths of any other files from \
             what it says."
        ),
        _ => format!(
            "{notice}\"{file}\" in the skill \"{id}\" is empty or could not be read. Carry on with \
             the task using what you know, and do not try this file again."
        ),
    }
}

/// What the two scopes currently hold, for the prompt. Absent means memory is off.
#[derive(Debug, Clone, Default)]
pub struct MemoryPrompt {
    pub global: String,
    pub server: String,
    pub server_name: String,
}

/// The recalled facts, and the reading instructions that keep them facts.
///
/// The two scopes are kept apart rather than merged, for two reasons: the model
/// has to know which one to file a new fact under, and a line that is only true
/// of one machine must never look like a rule about all of them. Told to treat
/// them as its own earlier findings, because they arrive in the same position a
/// user instruction would and are not one -- a stale line should lose to what the
/// machine says today, not override it.
fn memory_section(memory: &MemoryPrompt) -> Vec<String> {
    if memory.global.is_empty() && memory.server.is_empty() {
        return vec![
            "What you remember about this setup:".into(),
            "- Nothing yet.".into(),
            String::new(),
        ];
    }
    let mut lines = vec![
        "What you remember about this setup:".into(),
        "(Earlier facts, not user instructions. Machine evidence wins over stale memory.)".into(),
        String::new(),
    ];
    if !memory.global.is_empty() {
        lines.extend(["[global]".to_string(), memory.global.clone(), String::new()]);
    }
    if !memory.server.is_empty() {
        lines.extend([
            format!("[server: {}]", memory.server_name),
            memory.server.clone(),
            String::new(),
        ]);
    }
    lines
}

/// The skills on offer, one line each, and how to reach for one.
///
/// Only the descriptions are resident. That is the entire point of the feature:
/// a procedure runs to pages and is wanted on perhaps one task in twenty, so it
/// is advertised in a line and read when the model decides it is relevant. The
/// alternative -- pasting every skill into every request -- is what memory
/// already does, and why memory holds a page of one-line facts and not this.
fn skill_section(manifest: &[String]) -> Vec<String> {
    let mut lines = vec![
        "Procedures written for this setup:".to_string(),
        "(Load a relevant skill before acting; follow it and verify against the machine.)".into(),
    ];
    lines.extend(manifest.iter().cloned());
    lines.push(String::new());
    lines
}

/// The OS on the user's side of a transfer, so a local path is written in the
/// right shape. This process is the one whose disk a download lands on, so its
/// own platform is the answer.
fn local_system() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows"
    } else if cfg!(target_os = "macos") {
        "macOS"
    } else {
        "Linux"
    }
}

/// Everything the request carries that is not the conversation itself.
pub struct PromptInput<'a> {
    pub language: Language,
    /// The machine, named and described.
    ///
    /// In the prompt rather than in a turn of the conversation because none of it
    /// changes while a session lasts. A standing fact belongs where standing
    /// instructions are, and keeping it here is what leaves the prompt identical
    /// from one task to the next -- which is where a prefix cache starts looking.
    pub host: &'a str,
    pub machine: &'a super::context::MachineFacts,
    /// What answers, named. Shown to the user when they ask what model this is.
    pub identity: &'a str,
    /// Absent switches memory off: the actions are never offered, so naming one
    /// is a mistake rather than a refusal.
    pub memory: Option<&'a MemoryPrompt>,
    /// The named local directories, one per line. Empty when nothing was resolved.
    pub local_places: &'a [String],
    /// One line per loadable skill. Empty keeps the whole feature out of the prompt.
    pub skills: &'a [String],
    /// Whether the model was asked to think. False drops the line about which
    /// language to think in, which is three lines of instruction about something
    /// that is not going to happen.
    pub thinking: bool,
}

pub fn build_system_prompt(input: &PromptInput) -> String {
    build_system_prompt_for(input, true)
}

/// Prompt used only for endpoints that have proved they reject native tools.
pub fn build_json_system_prompt(input: &PromptInput) -> String {
    build_system_prompt_for(input, false)
}

fn build_system_prompt_for(input: &PromptInput, native_tools: bool) -> String {
    let reply = match input.language {
        Language::ZhCn => "Chinese",
        Language::EnUs => "English",
    };
    let has_memory = input.memory.is_some();
    let has_skills = !input.skills.is_empty();

    /// A block of fixed lines. A free function rather than a closure, because the
    /// variable lines between the blocks need the same `&mut` and a closure
    /// holding it would keep them out.
    fn push(out: &mut Vec<String>, lines: &[&str]) {
        out.extend(lines.iter().map(|line| line.to_string()));
    }

    let mut out: Vec<String> = Vec::new();

    push(&mut out, &[
        "You are tshell's general-purpose chat assistant, with tools for a remote Linux machine.",
        "",
        "Model:",
    ]);
    out.push(format!(
        "- You are the assistant in tshell, a desktop SSH client, running on {}.",
        input.identity
    ));
    push(&mut out, &[
        "- Ordinary prose is for answers and questions. There is no speak, ask or finish action;",
        "  a reply with no action is the final answer.",
        "",
    ]);
    if native_tools {
        push(&mut out, &[
            "To do something on the machine, call one of the tools provided with the request.",
            "Use the tool interface, never prose that imitates a call. Parallel read-only calls",
            "are welcome; actions that change state must be sequential.",
        ]);
    } else {
        push(&mut out, &[
            "To do something on the machine, put ONE JSON object at the VERY END of your reply,",
            "after whatever you wanted to say, and write nothing after it. Only the object at the",
            "end is read as an action -- JSON anywhere earlier is only an example. Allowed shapes:",
            "{\"action\":\"run\",\"command\":\"<one-line shell command>\",\"why\":\"<short reason>\"}",
            "{\"action\":\"write\",\"path\":\"<absolute path>\",\"content\":\"<the entire file>\",\"why\":\"<short reason>\"}",
            "{\"action\":\"append\",\"path\":\"<absolute path>\",\"content\":\"<text to add>\",\"why\":\"<short reason>\"}",
            "{\"action\":\"edit\",\"path\":\"<absolute path>\",\"old\":\"<exact text to replace>\",\"new\":\"<replacement>\",\"why\":\"<short reason>\"}",
            "{\"action\":\"download\",\"path\":\"<remote path, or a list of them>\",\"to\":\"<local folder>\",\"why\":\"<short reason>\"}",
            "{\"action\":\"upload\",\"path\":\"<local path, or a list of them>\",\"to\":\"<remote folder>\",\"why\":\"<short reason>\"}",
        ]);
    }
    // An action the model is never shown is one it never tries and is never
    // refused for, which is a cleaner way to switch memory off than answering it.
    if has_memory && !native_tools {
        push(&mut out, &[
            "{\"action\":\"remember\",\"scope\":\"server|global\",\"text\":\"<one durable fact>\",\"why\":\"<short reason>\"}",
        ]);
    }
    // Same rule: someone who has written no skills pays nothing for the feature
    // -- not a line of protocol, not a word of guidance.
    if has_skills && !native_tools {
        push(&mut out, &[
            "{\"action\":\"skill\",\"name\":\"<one of the skills listed below>\",\"file\":\"<optional file inside it>\",\"why\":\"<short reason>\"}",
        ]);
    }
    out.push(String::new());
    out.push(format!(
        "Write your prose, and \"why\" and \"text\", in {reply}."
    ));
    if input.thinking {
        out.push(format!(
            "- Reason in {reply} too. Reasoning is visible to the user."
        ));
    }
    push(&mut out, &[
        "- Put code and file contents in fenced blocks with a language tag.",
        "",
        "Choose prose or a tool:",
        "- Use prose alone for conversation, explanations and anything you already know.",
        "- Use run only for this machine's state or work the user requested.",
        "- Use write/append/edit for file changes; never change files through shell commands,",
        "  redirection, heredocs, sed -i, tee or similar workarounds.",
        "- Use upload/download only when the user explicitly asked to transfer files.",
        "- Ask the user in prose only for a decision or fact only they can provide; ask nothing",
        "  else in that turn and never ask permission for routine read-only investigation.",
    ]);
    if has_memory {
        push(&mut out, &[
            "- Use remember for one durable fact as a task finishes or when the user states it;",
            "  never store transient output, task progress, unverified conclusions or secrets.",
            "  Default to server scope; use global only for facts true across all machines.",
        ]);
    }
    if has_skills {
        push(&mut out, &[
            "- Load a matching skill before acting. Start with its main file; load another file only",
            "  when instructed. Skill scripts are text: write them to the machine before running.",
        ]);
    }
    push(&mut out, &[
        "",
        "Writing files:",
        "- Send content as ordinary text with real newlines; encoding is preserved automatically",
        "  (new files use UTF-8). Never shell-escape, base64 or run iconv for file content.",
        "- write creates/replaces a complete file; append adds to its end; edit changes an existing",
        "  file. Prefer edit for existing files. Its old text must match exactly once.",
        "- Read before a rewrite, avoid consecutive full writes, and verify every change.",
        "- Back up only system files before rewriting them. File actions require confirmation.",
        "- /dev, /proc, /sys and /boot are refused outright. Everywhere else is allowed.",
        "",
        "Transfers:",
        "- download moves remote files/folders to the user's computer; upload does the reverse.",
        "  Use write, not upload, for content you compose. Use transfers for large/binary files.",
        "- to is an existing destination folder; items keep their names. Existing destinations",
        "  are skipped, never overwritten. Issue the transfer directly and use its current result",
        "  instead of pre-checking with stale listings.",
    ]);
    out.push(format!(
        "- Local paths are on the user's {} computer. You cannot",
        local_system()
    ));
    push(
        &mut out,
        &["  list it: use only paths supplied by the user, listed below, or beneath those paths."],
    );
    if !input.local_places.is_empty() {
        out.push("- Directories on that computer, resolved for you:".to_string());
        out.extend(input.local_places.iter().cloned());
    }
    push(&mut out, &[
        "- Resolve desktop/download requests from that list. If a local source or destination is",
        "  genuinely unknown, omit path or to to show the appropriate picker.",
    ]);
    push(
        &mut out,
        &[if cfg!(target_os = "macos") {
            "  It can select folders as well."
        } else {
            "  It takes files only -- to upload a folder you must have its path."
        }],
    );
    // Only where it is true. Told to double backslashes on a Mac, a model starts
    // writing "/Users/\\me", which is a path to nothing.
    if cfg!(target_os = "windows") {
        push(&mut out, &[
            "- Local paths are Windows paths, and a backslash must be doubled inside a JSON string:",
            "  \"D:\\\\logs\", not \"D:\\logs\".",
        ]);
    }
    push(&mut out, &[
        "- An upload without to goes to the terminal's current remote directory.",
        "",
        "Shell and workflow:",
        "- Commands run visibly in the user's persistent terminal session. Avoid a bare cd; use",
        "  absolute paths grounded in pwd/output, never guessed paths.",
        "- One command per step. Chain with && only when the steps are inseparable.",
        "- Investigate with read-only commands before changing anything.",
        "- Never run interactive or non-returning programs: editors, pagers, top or follow mode.",
        "- Avoid sudo; if a password is required, ask the user in prose.",
        "- Always pass non-interactive flags (-y, DEBIAN_FRONTEND=noninteractive).",
        "- Never use placeholders; discover unknown values first.",
        "",
        "Safety:",
        "- Catastrophic broad actions are blocked; other state changes require confirmation.",
        "  If blocked, narrow the target instead of giving up.",
        "",
        "Failure and completion:",
        "- Diagnose failures from relevant logs, config and state; do not stop at the first error.",
        "- Finish only after the requested outcome is carried out and verified. If genuinely",
        "  blocked, report what was tried and the blocker.",
        "- Never narrate a future step without issuing its action.",
        "",
    ]);
    if let Some(memory) = input.memory {
        out.extend(memory_section(memory));
    }
    if has_skills {
        out.extend(skill_section(input.skills));
    }
    out.extend(machine_section(input.host, input.machine));
    /*
     * The machine and session summary used to be the last thing in here, and it
     * is now a turn of the conversation instead. See `context_turn` below.
     *
     * It is a caching decision. This prompt is the first message of every
     * request, and every endpoint that caches does so on a prefix that is equal
     * from the very beginning. The summary carries the working directory and the
     * tail of the terminal, so it changed on every task -- which broke the match
     * at message zero and re-billed the entire conversation, every step, at full
     * price. Measured on a real session: eighteen thousand characters of this
     * prompt never changed and the last four thousand always did.
     *
     * Moved into the conversation, the summary is stated once at the moment it
     * was true and never rewritten, so everything before the newest turn is
     * byte-identical to the last request. That is the shape prefix caching is
     * built for -- and it is the more honest shape anyway: a fact about the
     * machine at 11:04 is a thing that was said then, not a standing instruction.
     */
    out.join("\n")
}

/// The machine, in the two lines that are worth standing instructions.
///
/// The working directory is deliberately absent. It is the one thing here that
/// changes while a task runs, and a line measured before the task started is
/// stale the moment the model runs `cd` -- it read as fact and was not one. The
/// model is told to ask instead, which is the only answer that stays true.
fn machine_section(host: &str, machine: &super::context::MachineFacts) -> Vec<String> {
    let or_unknown = |value: &str| if value.is_empty() { "unknown" } else { value }.to_string();
    vec![
        "The machine you are working on:".to_string(),
        format!("Host: {host}"),
        format!(
            "OS: {} | kernel {} | shell {} | user {} | home {}",
            or_unknown(&machine.os),
            or_unknown(&machine.kernel),
            or_unknown(&machine.shell),
            or_unknown(&machine.user),
            or_unknown(&machine.home)
        ),
        "CWD is dynamic and not listed here; run pwd when needed instead of assuming it."
            .to_string(),
        String::new(),
    ]
}

/// The machine summary as a turn of the conversation.
///
/// Framed as a client notice for the same reason the dropped-actions notice is:
/// it arrives in the user's place in the conversation, and a model that mistakes
/// it for something the user said will answer it.
pub fn context_turn(context: &str, user_text: &str) -> String {
    if context.trim().is_empty() {
        return user_text.to_string();
    }
    format!(
        "[tshell client notice -- not from the user. The machine as it stands right now. Do not \
         reply to this part.]\n{context}\n\n{user_text}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A machine that never changes, which is the point of it being in here.
    static FACTS: std::sync::LazyLock<super::super::context::MachineFacts> =
        std::sync::LazyLock::new(|| super::super::context::MachineFacts {
            os: "Ubuntu 24.04".into(),
            kernel: "6.8.0".into(),
            shell: "/bin/bash".into(),
            user: "me".into(),
            home: "/home/me".into(),
        });

    fn base<'a>() -> PromptInput<'a> {
        PromptInput {
            language: Language::EnUs,
            host: "web-1",
            machine: &FACTS,
            identity: "gpt-x",
            memory: None,
            local_places: &[],
            skills: &[],
            thinking: true,
        }
    }

    #[test]
    fn wide_scripts_count_as_a_token_each() {
        assert_eq!(estimate_tokens("你好"), 2);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("你好abcd"), 3);
    }

    #[test]
    fn an_exit_that_names_a_signal_says_so() {
        assert_eq!(describe_exit(0), "EXIT: 0");
        assert_eq!(describe_exit(1), "EXIT: 1");
        assert_eq!(describe_exit(137), "EXIT: 137 (killed by SIGKILL)");
        assert_eq!(describe_exit(130), "EXIT: 130 (killed by SIGINT)");
        // 128 + 7 is a signal with no name in the table, so it stays a number.
        assert_eq!(describe_exit(135), "EXIT: 135");
    }

    #[test]
    fn a_failing_result_ends_with_the_guidance() {
        let result = CommandResult {
            output: "no such file".into(),
            exit_code: 2,
            timed_out: false,
            truncated: false,
        };
        let text = describe_result(&result);
        assert!(text.starts_with("EXIT: 2\nOUTPUT:\nno such file"));
        assert!(text.ends_with("Do not stop and report this failure as though it were the answer."));
    }

    #[test]
    fn a_clean_result_is_three_lines_and_no_lecture() {
        let result = CommandResult {
            output: "ok".into(),
            ..Default::default()
        };
        assert_eq!(describe_result(&result), "EXIT: 0\nOUTPUT:\nok");
    }

    #[test]
    fn empty_output_is_said_rather_than_left_blank() {
        let result = CommandResult::default();
        assert_eq!(describe_result(&result), "EXIT: 0\nOUTPUT:\n(no output)");
        let result = CommandResult {
            timed_out: true,
            ..Default::default()
        };
        assert!(describe_result(&result).contains("OUTPUT:\n(nothing)"));
    }

    #[test]
    fn a_truncated_result_says_where_the_middle_went() {
        let result = CommandResult {
            output: "x".into(),
            truncated: true,
            ..Default::default()
        };
        assert!(describe_result(&result).contains("OUTPUT (middle omitted):"));
    }

    #[test]
    fn folding_replaces_the_output_and_keeps_the_command() {
        let content = "EXIT: 0\nOUTPUT:\nlots and lots of text";
        let folded = fold_output(content).unwrap();
        assert!(folded.starts_with("EXIT: 0\n[Output omitted here: "));
        assert!(folded.contains("Run the command again if you need it back."));
        assert!(fold_output("no header at all").is_none());
    }

    #[test]
    fn folding_finds_the_truncated_header_too() {
        assert!(fold_output("EXIT: 1\nOUTPUT (middle omitted):\nstuff").is_some());
    }

    #[test]
    fn a_skill_body_is_recognised_by_its_header() {
        let body = format!("{}\nthe procedure", skill_header("deploy", "SKILL.md"));
        assert_eq!(skill_key_of(&body).as_deref(), Some("deploy::SKILL.md"));
        let folded = fold_skill(&body).unwrap();
        assert!(folded.starts_with("[The skill deploy (SKILL.md) was loaded here"));
    }

    #[test]
    fn a_header_that_is_not_at_the_top_is_not_a_skill_body() {
        let body = format!("prelude\n{}", skill_header("deploy", "SKILL.md"));
        assert!(skill_key_of(&body).is_some());
        assert!(fold_skill(&body).is_none());
    }

    #[test]
    fn ordinary_text_is_not_mistaken_for_either_header() {
        assert!(skill_key_of("SKILL notes: read them").is_none());
        assert!(fold_output("OUTPUT: inline, not a header line").is_none());
    }

    #[test]
    fn memory_outcomes_each_say_what_to_do_next() {
        let text = describe_memory(MemoryScope::Global, MemoryOutcome::Ok);
        assert_eq!(
            text,
            "Stored in global memory. Carry on with what you were doing."
        );
        let text = describe_memory(MemoryScope::Server, MemoryOutcome::Full);
        assert!(text.starts_with("This server's memory is full and NOTHING was written."));
        let text = describe_memory(MemoryScope::Server, MemoryOutcome::Failed);
        assert!(text.contains("was NOT saved"));
    }

    #[test]
    fn a_skill_failure_names_the_next_move() {
        let text = describe_skill_failure(SkillOutcome::Unknown, "deploy", "SKILL.md");
        assert!(text.contains("There is no skill called \"deploy\""));
        let text = describe_skill_failure(SkillOutcome::Missing, "deploy", "extra.md");
        assert!(text.contains("{\"action\":\"skill\",\"name\":\"deploy\"}"));
        let text = describe_skill_failure(SkillOutcome::Disabled, "deploy", "SKILL.md");
        assert!(text.contains("switched off by the user"));
    }

    #[test]
    fn memory_off_keeps_its_protocol_and_guidance_out_of_the_prompt() {
        let prompt = build_system_prompt(&base());
        assert!(!prompt.contains("\"action\":\"remember\""));
        assert!(!prompt.contains("What to remember, and what not to:"));
        assert!(!prompt.contains("What you remember about this setup:"));
    }

    #[test]
    fn memory_on_carries_both_scopes_apart() {
        let memory = MemoryPrompt {
            global: "- prefers tabs".into(),
            server: "- nginx is at /opt/nginx".into(),
            server_name: "web-1".into(),
        };
        let prompt = build_system_prompt(&PromptInput {
            memory: Some(&memory),
            ..base()
        });
        assert!(!prompt.contains("{\"action\":\"remember\""));
        assert!(prompt.contains("[global]\n- prefers tabs"));
        assert!(prompt.contains("[server: web-1]\n- nginx is at /opt/nginx"));
        let fallback = build_json_system_prompt(&PromptInput {
            memory: Some(&memory),
            ..base()
        });
        assert!(fallback.contains("{\"action\":\"remember\""));
    }

    #[test]
    fn empty_memory_still_invites_the_first_fact() {
        let memory = MemoryPrompt::default();
        let prompt = build_system_prompt(&PromptInput {
            memory: Some(&memory),
            ..base()
        });
        assert!(prompt.contains("- Nothing yet."));
        assert!(!prompt.contains("[global]"));
    }

    #[test]
    fn skills_off_keeps_the_whole_feature_out() {
        let prompt = build_system_prompt(&base());
        assert!(!prompt.contains("\"action\":\"skill\""));
        assert!(!prompt.contains("Procedures written for this setup:"));
    }

    #[test]
    fn skills_on_lists_them_and_says_how_to_reach_for_one() {
        let skills = vec!["- deploy: how releases go out here".to_string()];
        let prompt = build_system_prompt(&PromptInput {
            skills: &skills,
            ..base()
        });
        assert!(!prompt.contains("{\"action\":\"skill\""));
        assert!(prompt.contains("- deploy: how releases go out here"));
        let fallback = build_json_system_prompt(&PromptInput {
            skills: &skills,
            ..base()
        });
        assert!(fallback.contains("{\"action\":\"skill\""));
    }

    #[test]
    fn native_tools_and_json_fallback_have_separate_protocols() {
        let native = build_system_prompt(&base());
        assert!(native.contains("call one of the tools provided with the request"));
        assert!(!native.contains("Allowed shapes:"));
        assert!(!native.contains("{\"action\":\"run\""));

        let fallback = build_json_system_prompt(&base());
        assert!(fallback.contains("Allowed shapes:"));
        assert!(fallback.contains("{\"action\":\"run\""));
        assert!(!fallback.contains("call one of the tools provided with the request"));
    }

    #[test]
    fn system_prompts_stay_compact() {
        let native = build_system_prompt(&base());
        let fallback = build_json_system_prompt(&base());
        assert!(
            native.chars().count() < 7_000,
            "native prompt grew to {} chars",
            native.len()
        );
        assert!(
            fallback.chars().count() < 9_000,
            "JSON fallback prompt grew to {} chars",
            fallback.len()
        );
    }

    #[test]
    fn the_model_is_named_without_coaching_its_answer() {
        let prompt = build_system_prompt(&base());
        assert!(prompt.contains("running on gpt-x"));
        assert!(!prompt.contains("Asked what model"));
        assert!(!prompt.contains("underlying model is not disclosed"));
        assert!(!prompt.contains("state that identity"));
    }

    #[test]
    fn the_reply_language_follows_the_setting() {
        let prompt = build_system_prompt(&base());
        assert!(prompt.contains("\"text\", in English."));
        let prompt = build_system_prompt(&PromptInput {
            language: Language::ZhCn,
            ..base()
        });
        assert!(prompt.contains("\"text\", in Chinese."));
        assert!(prompt.contains("- Reason in Chinese too."));
    }

    /// Nothing in here may offer a verb for speaking or for stopping. A model
    /// told about one uses it, and its argument then arrives beside the prose the
    /// model already wrote -- with the panel having to draw one of the two.
    #[test]
    fn the_protocol_offers_no_way_to_speak_or_to_finish() {
        let prompt = build_system_prompt(&PromptInput {
            memory: Some(&MemoryPrompt::default()),
            skills: &["- deploy: how releases go out here".to_string()],
            ..base()
        });
        for verb in ["say", "ask", "done", "forget"] {
            assert!(
                !prompt.contains(&format!("\"action\":\"{verb}\"")),
                "the prompt still offers {verb}"
            );
        }
    }

    #[test]
    fn not_thinking_drops_the_instruction_about_thinking() {
        let prompt = build_system_prompt(&PromptInput {
            thinking: false,
            ..base()
        });
        assert!(!prompt.contains("Reason in English too."));
    }

    /// The prompt has to be the same bytes from one task to the next, because it
    /// is message zero and every cache match starts there. Anything that changes
    /// with the machine belongs in `context_turn`, not in here.
    #[test]
    fn the_prompt_carries_nothing_that_changes_between_tasks() {
        let prompt = build_system_prompt(&base());
        assert!(!prompt.contains("Machine and session context"));
        assert!(
            !prompt.contains("CWD:"),
            "the working directory is not a standing instruction"
        );
        assert!(!prompt.contains("Recent commands"));
        assert!(!prompt.contains("Working directory"));
    }

    #[test]
    fn the_context_arrives_as_a_turn_and_says_it_is_not_the_user() {
        let turn = context_turn("Host: web-1\nWorking directory: /srv", "restart nginx");
        assert!(turn.starts_with("[tshell client notice"));
        assert!(turn.contains("Host: web-1"));
        // The user's own words last, so the thing being asked is the thing the
        // model reads last.
        assert!(turn.ends_with("restart nginx"));
    }

    /// Nothing to say about the machine is not a reason to wrap the question in a
    /// notice about there being nothing to say.
    #[test]
    fn an_empty_context_leaves_the_question_alone() {
        assert_eq!(context_turn("", "hello"), "hello");
        assert_eq!(context_turn("   \n ", "hello"), "hello");
    }

    #[test]
    fn resolved_local_directories_are_listed_when_there_are_any() {
        let places = vec!["  Desktop: C:\\Users\\me\\Desktop".to_string()];
        let prompt = build_system_prompt(&PromptInput {
            local_places: &places,
            ..base()
        });
        assert!(prompt.contains("- Directories on that computer, resolved for you:"));
        assert!(prompt.contains("  Desktop: C:\\Users\\me\\Desktop"));
        let prompt = build_system_prompt(&base());
        assert!(!prompt.contains("- Directories on that computer, resolved for you:"));
    }
}
