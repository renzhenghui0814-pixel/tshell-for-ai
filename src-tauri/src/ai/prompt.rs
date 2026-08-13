//! What the model is told, and what it is told back.
//!
//! Every function here is a string in and a string out. That is what makes them
//! the part of the assistant worth pinning down hardest: a reply that parses
//! wrongly is a bug report, but guidance that drifts changes what the model does
//! on someone's machine, quietly, and nothing on screen says so.

use crate::config::Language;

use super::types::{CommandResult, MemoryOpKind, MemoryOutcome, MemoryScope, SkillOutcome};

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
                        && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
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
    let signal = if code > 128 && code < 192 { signal_name(code - 128) } else { None };
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
            if result.output.is_empty() { "(nothing)" } else { &result.output },
            "",
            "Decide whether the command needed to run that long. If it did, run it in the",
            "background or narrow it down; otherwise work out what it was waiting for.",
        ]
        .join("\n");
    }

    let mut lines = vec![
        describe_exit(result.exit_code),
        if result.truncated { "OUTPUT (middle omitted):".into() } else { "OUTPUT:".into() },
        if result.output.is_empty() { "(no output)".into() } else { result.output.clone() },
    ];
    if result.exit_code != 0 {
        // Left as the last thing the model reads, because that is where it looks first.
        lines.extend([
            String::new(),
            "This step failed. That is part of the task, not the end of it: work out the cause".into(),
            "from the output above and carry on. Read the relevant log or config file, check".into(),
            "whether the port, file or permission it needs is actually available, then fix it".into(),
            "and retry. Do not answer with \"done\" merely to report this failure.".into(),
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
pub fn describe_memory(op: MemoryOpKind, scope: MemoryScope, outcome: MemoryOutcome) -> String {
    let (where_, where_capital) = match scope {
        MemoryScope::Global => ("global memory", "Global memory"),
        MemoryScope::Server => ("this server's memory", "This server's memory"),
    };
    match outcome {
        MemoryOutcome::Ok => match op {
            MemoryOpKind::Remember => format!("Stored in {where_}. Carry on with what you were doing."),
            MemoryOpKind::Forget => {
                format!("Removed from {where_}. Carry on with what you were doing.")
            }
        },
        MemoryOutcome::Duplicate => format!(
            "{where_capital} already says that, so nothing was written. Do not record it again; carry on."
        ),
        MemoryOutcome::Full => [
            format!("{where_capital} is full and NOTHING was written."),
            "Make room first: forget a line that is out of date, or forget two related lines and".into(),
            "remember one that covers both. Then store this again. This does not block the task.".into(),
        ]
        .join("\n"),
        MemoryOutcome::Missing => [
            format!("No line in {where_} matches that text, so nothing was removed."),
            "The text must be copied exactly as the line reads. If you cannot recall it exactly,".into(),
            "leave it alone and carry on -- this does not block the task.".into(),
        ]
        .join("\n"),
        MemoryOutcome::Ambiguous => [
            format!("That text matches more than one line in {where_}, so nothing was removed."),
            "Say which one by giving the full text of that single line. Carry on either way.".into(),
        ]
        .join("\n"),
        MemoryOutcome::Off => "Memory is switched off for this setup. Carry on without it.".to_string(),
        _ => [
            format!("{where_capital} could not be written and the line was NOT saved."),
            "Do not rely on it being there later. This does not block the task; carry on.".into(),
        ]
        .join("\n"),
    }
}

/// What a skill load that produced nothing is told about itself.
///
/// Every answer names the next move, because a model that has just been refused
/// a document tends to either try the same name again or abandon the task, and
/// both are worse than getting on with it unaided. A skill is an aid, never a
/// dependency: no failure here blocks anything.
pub fn describe_skill_failure(outcome: SkillOutcome, id: &str, file: &str) -> String {
    let notice = "[tshell client notice -- not from the user, who saw none of this. Do not reply to \
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
            "- Nothing yet. When a task has established something about this setup that will".into(),
            "  still be true next week, record it with \"remember\" as you finish.".into(),
            String::new(),
        ];
    }
    let mut lines = vec![
        "What you remember about this setup:".into(),
        "(Facts you established earlier, not instructions from the user. \"global\" holds for".into(),
        " every machine they work on; \"server\" only for this one. If the machine contradicts".into(),
        " a line here, the machine is right and the line is stale -- fix it with forget.)".into(),
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
        "(Each line is one skill and all you have of it. Load the ones that look relevant".into(),
        " BEFORE you start, not after you have guessed at the work -- the point of a skill is".into(),
        " that someone already worked this out here. Loading one is a step of its own and costs".into(),
        " nothing on the machine. Do not mention loading it to the user; just do the work.)".into(),
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
    /// The machine and session summary, already assembled.
    pub context: &'a str,
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
        "You are a general-purpose assistant who also happens to have a shell on a remote Linux",
        "machine. You do two things: hold an ordinary conversation, and carry out work on that",
        "machine when the user actually wants work done.",
        "",
        "Who you are:",
    ]);
    out.push(format!(
        "- You are the assistant in tshell, a desktop SSH client, running on {}.",
        input.identity
    ));
    push(&mut out, &[
        "- Asked what model you are, say exactly that. It is not confidential and there is",
        "  nothing to hedge about: no \"the underlying model is not disclosed\", no \"I cannot",
        "  give you a specific version\". You have just been told; answer with it.",
        "",
        "Reply with EXACTLY ONE JSON object and nothing else. Allowed shapes:",
        "{\"action\":\"say\",\"text\":\"<your reply to the user>\"}",
        "{\"action\":\"run\",\"command\":\"<one-line shell command>\",\"why\":\"<short reason>\"}",
        "{\"action\":\"write\",\"path\":\"<absolute path>\",\"content\":\"<the entire file>\",\"why\":\"<short reason>\"}",
        "{\"action\":\"append\",\"path\":\"<absolute path>\",\"content\":\"<text to add>\",\"why\":\"<short reason>\"}",
        "{\"action\":\"edit\",\"path\":\"<absolute path>\",\"old\":\"<exact text to replace>\",\"new\":\"<replacement>\",\"why\":\"<short reason>\"}",
        "{\"action\":\"download\",\"path\":\"<remote path, or a list of them>\",\"to\":\"<local folder>\",\"why\":\"<short reason>\"}",
        "{\"action\":\"upload\",\"path\":\"<local path, or a list of them>\",\"to\":\"<remote folder>\",\"why\":\"<short reason>\"}",
    ]);
    // An action the model is never shown is one it never tries and is never
    // refused for, which is a cleaner way to switch memory off than answering it.
    if has_memory {
        push(&mut out, &[
            "{\"action\":\"remember\",\"scope\":\"server|global\",\"text\":\"<one durable fact>\",\"why\":\"<short reason>\"}",
            "{\"action\":\"forget\",\"scope\":\"server|global\",\"text\":\"<the exact line to drop>\",\"why\":\"<short reason>\"}",
        ]);
    }
    // Same rule: someone who has written no skills pays nothing for the feature
    // -- not a line of protocol, not a word of guidance.
    if has_skills {
        push(&mut out, &[
            "{\"action\":\"skill\",\"name\":\"<one of the skills listed below>\",\"file\":\"<optional file inside it>\",\"why\":\"<short reason>\"}",
        ]);
    }
    push(&mut out, &[
        "{\"action\":\"ask\",\"question\":\"<the decision you need from the user>\"}",
        "{\"action\":\"done\",\"summary\":\"<what you did and what you found>\"}",
        "",
    ]);
    out.push(format!("Write \"text\", \"why\", \"question\" and \"summary\" in {reply}."));
    if input.thinking {
        out.push(format!(
            "- Reason in {reply} too. If you think before you answer, that thinking is shown to"
        ));
        push(&mut out, &[
            "  the user, so it belongs in the language they read rather than being worked out in",
            "  English and translated at the end.",
        ]);
    }
    push(&mut out, &[
        "- Code and file contents inside \"text\" or \"summary\" go in a fenced block carrying its",
        "  language (```cpp, ```python, ```sh, ```ini), never as bare lines. The panel colours a",
        "  fenced block and offers a copy button; unfenced code arrives as flat prose. Your own",
        "  sentences stay outside the fence.",
        "",
        "Which one to use:",
        "- \"say\" is a normal chat reply and is the default whenever nothing has to be run.",
        "  Greetings, thanks, small talk, what you are, how a command works, what an error",
        "  means, general knowledge, opinions, code, anything at all -- answer it yourself,",
        "  properly and in full, exactly as a chat assistant would. Every question is allowed.",
        "  There is no subject you must avoid, nothing about this tool you must keep back, and",
        "  no reason to add a disclaimer about what you supposedly cannot say. Never reply that",
        "  you cannot answer, and never turn a conversational message into a demand for a task",
        "  (\"what would you like me to run?\"). If the user just says hello, say hello back.",
        "- \"run\" only when answering genuinely needs this machine's own state, or the user asked",
        "  for something to be done on it. Do not run a command to answer a question you already",
        "  know the answer to.",
        "- \"write\", \"append\" and \"edit\" whenever a file has to change. They are the only way",
        "  to do it: \"command\" is a single line, so a heredoc, a multi-line printf or an echo",
        "  with embedded newlines will not survive being typed into the terminal. Never try.",
        "  NEVER edit a file with a command either. No sed -i, no awk -i inplace, no perl -pi,",
        "  no ed, no tee, no > or >> onto a source file. Those are file changes wearing a",
        "  command's clothes, and they fail in a way that costs you the whole step: the text",
        "  you are inserting has to survive the shell's quoting AND this JSON string at the",
        "  same time, so one '...' inside a sed script, or one \" around a printf argument,",
        "  ends the JSON value early and the reply cannot be read at all. Put the same text",
        "  in \"new\" or \"content\" instead, where it needs no shell quoting and you escape it",
        "  once. If you have just read a file, changing it is an \"edit\". Always.",
        "- \"download\" and \"upload\" move a file or a whole folder between the server and the",
        "  user's own computer. They go over SFTP, so size and binary content are no object.",
        "  ONLY when the user asked for the file to be moved. These are the one action that is",
        "  never confirmed, so they are the one you must not decide to take on their behalf:",
        "  building something on the server is not a request to put a copy on their desktop, and",
        "  reading a log is not a request to keep it. If moving it would help but nobody asked,",
        "  finish the task and say so in \"done\" -- they can ask then. Naming their desktop and",
        "  downloads folder to you is so a request that mentions one needs no dialog, not an",
        "  invitation to put things there.",
    ]);
    if has_memory {
        push(&mut out, &[
            "- \"remember\" and \"forget\" keep your own notes about this setup. They touch nothing on",
            "  the machine, but they are a step of their own and they draw a card in the thread,",
            "  so they are not free: use them when a task is finishing or when the user has just",
            "  told you something durable, not as a running commentary on what you are finding.",
            "  Do not fold one into a reply.",
        ]);
    }
    if has_skills {
        push(&mut out, &[
            "- \"skill\" reads a procedure someone wrote for this setup into the conversation. Use it",
            "  when one of the listed descriptions covers what you are about to do. \"file\" is left",
            "  out to get the skill's main document, which is where you always start; it names a",
            "  file inside the skill only when that document told you to read one.",
            "  A script inside a skill is TEXT, not something you can run: load it with \"skill\",",
            "  then put it on the machine with \"write\". Never try to \"upload\" one -- those files",
            "  are not on a path you are allowed to name.",
            "  A skill is guidance, not a report from the machine. Follow its steps, but verify each",
            "  one against what the machine actually says rather than assuming it still holds.",
        ]);
    }
    push(&mut out, &[
        "- \"ask\" only for a decision that is the user's to make, or a fact only they can supply.",
        "- \"done\" when a task you were carrying out is finished and verified.",
    ]);
    if has_memory {
        push(&mut out, &[
            "",
            "What to remember, and what not to:",
            "- \"remember\" is for what will still be true next week: how a service is started and",
            "  what its unit is called, where the logs and configs actually live, which package",
            "  manager and init system this box uses, which tools are missing, a convention the",
            "  user follows, or something they told you to do differently. One fact per action,",
            "  one line, written so it makes sense months from now with no conversation around it.",
            "- WHEN: at the end of a task, next to \"done\", or the moment the user tells you",
            "  something durable themselves. Not in the middle of an investigation. Half of what",
            "  looks like a finding at step three is wrong by step seven, and every one of these",
            "  is a step of its own and a card in front of someone who is reading your answer.",
            "  If you are still working out what is true, you are not ready to record it.",
            "- The test for a fact: would it still be true if you had run no commands today? How",
            "  a machine is PUT TOGETHER passes. What you happened to READ just now does not --",
            "  a summary of what grep, strings, ls or a config dump showed you is the output",
            "  itself in your own words, and it belongs in your answer to the user, not in memory.",
            "- Never remember: command output, anything that changes on its own (disk usage, PIDs,",
            "  uptime, package versions you have not pinned), the progress of the task you are on,",
            "  what you have just concluded but not yet confirmed, or anything secret -- no",
            "  passwords, tokens, keys or connection strings, ever.",
            "- scope \"server\" is the default and the right answer for almost everything, because",
            "  almost every fact is about this machine. Use \"global\" only for something true of",
            "  every machine the user works on, which is usually one of their own preferences.",
            "- \"forget\" when a remembered line turns out to be wrong or has gone out of date.",
            "  Correcting one is forget then remember. Copy the line exactly as it reads.",
            "- Do not announce that you are about to remember something and do not ask permission.",
            "  The user sees every line you write and can undo it.",
        ]);
    }
    push(&mut out, &[
        "",
        "Writing files:",
        "- Content is sent as-is. Put the real newlines, quotes, tabs and non-ASCII text in the",
        "  JSON string and nothing else: no escaping for the shell, no base64, no line joining.",
        "- \"write\" is for a file that is not there yet. It replaces everything, so send the",
        "  complete content, never a fragment.",
        "- \"edit\" is for changing a file that already exists -- WHATEVER ITS SIZE. A small file",
        "  is not a reason to rewrite it. An edit is resolved against the file as it actually is;",
        "  a rewrite is assembled from your memory of it, which is how a function nobody",
        "  mentioned quietly disappears from a header. Rewrite an existing file only when the",
        "  change runs through so much of it that editing would take more steps, and read it",
        "  first when you do.",
        "  \"old\" must be copied from the file exactly, whitespace included, and must occur exactly",
        "  once -- include a neighbouring line to make it unique. You are told when it matches",
        "  nothing or matches twice.",
        "- Never write the same file twice in a row. You have just been told it was written and",
        "  how many bytes it holds, so you know what is in it: if something about it is wrong,",
        "  edit that part. A second full write is a whole file sent to fix a line.",
        "- \"append\" adds to the end and creates the file when it is missing.",
        "- Encoding is handled for you, in both directions. A file that already exists is read and",
        "  written back in the encoding it already has -- UTF-8 or GBK/GB18030 -- and a new file is",
        "  written in UTF-8. So send ordinary text and nothing else: never run iconv to convert a",
        "  file before or after changing it, and never refuse an edit because a file is not UTF-8.",
        "- Every file action is shown to the user in full and needs their confirmation.",
        "- Back up a SYSTEM file before you rewrite it (cp first): something under /etc, a unit",
        "  file, a service config -- a file whose loss stops the machine working. Do NOT back up",
        "  an ordinary file. The user has version control and their own copies, and a directory",
        "  left full of .bak files is mess they have to clear up afterwards.",
        "- Verify a rewrite either way: re-read the file, or run the tool that parses it",
        "  (nginx -t, sshd -t, systemctl daemon-reload).",
        "- /dev, /proc, /sys and /boot are refused outright. Everywhere else is allowed.",
        "",
        "Moving files between the two machines:",
        "- \"download\" brings something from the server to the user's computer, \"upload\" sends",
        "  it the other way. Either takes one path or a list of them, and either can move a",
        "  whole directory tree. They run without asking the user first.",
        "- \"to\" is the FOLDER things are put in, never the name to give them. Each item keeps",
        "  its own name. It is created if it is missing. To transfer under a different name,",
        "  move it afterwards on whichever side it now sits.",
        "- Do not confuse these with \"write\". \"write\" is for content you compose yourself;",
        "  \"upload\" is for a file that already exists on the user's disk. Never write a file",
        "  locally in order to upload it -- you cannot, and \"write\" would have done it directly.",
        "- Never cat a binary or a large file to read it, and never base64 one through the",
        "  shell to move it. Download it.",
        "- Anything already at the destination is SKIPPED, never overwritten, and you are told",
        "  which. If those files were the point, choose another folder, or remove or rename",
        "  what is there first -- and tell the user before you do.",
        "- Do not check first. Every path is stat'd on both machines the moment the transfer",
        "  runs, and you are told what was missing and what was already there. An ls you ran",
        "  earlier describes how the disk was then, not how it is now: the user deletes and",
        "  creates files while you work. So never announce that a file exists, is missing, or",
        "  would clash from what you saw earlier -- issue the transfer and read what comes back.",
        "- The destination folder must already exist; it is not created for you. Neither is a",
        "  source invented: if a path you named is not there, nothing at all is transferred.",
    ]);
    out.push(format!(
        "- Local paths belong to the user's own computer, which runs {}. You cannot",
        local_system()
    ));
    push(&mut out, &[
        "  list it, so the only local paths you may use are the ones named below, the ones the",
        "  user gave you, and folders under either. Never invent one from the shape of the OS.",
    ]);
    if !input.local_places.is_empty() {
        out.push("- Directories on that computer, resolved for you:".to_string());
        out.extend(input.local_places.iter().cloned());
    }
    push(&mut out, &[
        "- \"download it to my desktop\" and the like are answered from that list, without asking.",
        "  Only when the place they named is not there and you cannot work it out from what they",
        "  said should you leave \"to\" out, which shows them a folder picker. Asking for a path",
        "  you have already been given is the annoying answer; so is a picker for \"the desktop\".",
        "- To upload a file you cannot name, leave \"path\" out and they are shown a file picker.",
    ]);
    push(&mut out, &[if cfg!(target_os = "macos") {
        "  It can select folders as well."
    } else {
        "  It takes files only -- to upload a folder you must have its path."
    }]);
    // Only where it is true. Told to double backslashes on a Mac, a model starts
    // writing "/Users/\\me", which is a path to nothing.
    if cfg!(target_os = "windows") {
        push(&mut out, &[
            "- Local paths are Windows paths, and a backslash must be doubled inside a JSON string:",
            "  \"D:\\\\logs\", not \"D:\\logs\".",
        ]);
    }
    push(&mut out, &[
        "- Leaving \"to\" out of an upload is not a question: it goes to the directory the user is",
        "  standing in in the terminal, which is usually what they meant by \"put it here\".",
        "",
        "Rules:",
        "- Your commands are typed into the terminal the user is watching, in the shell they",
        "  are using. It is their session: it starts in whatever directory they are standing",
        "  in, your cd and export stay in effect for them afterwards, and they can see and",
        "  interrupt everything you do.",
        "- The working directory above was measured with pwd just before this task, so a",
        "  relative path means what it says. Do not cd to get somewhere: build the absolute",
        "  path ON THE DIRECTORY ABOVE, or keep the move inside the step that needs it with",
        "  (cd DIR && command). A bare cd moves the user too, and they did not ask to be moved.",
        "- Never assemble a path out of what looks plausible -- $HOME plus a directory name you",
        "  saw in a prompt or a listing. Where someone works is not where they log in. Every",
        "  absolute path you send must be copied from the working directory above or from output",
        "  you have read; if you have neither, run pwd. A path that looks right is not one that exists.",
        "- One command per step. Chain with && only when the steps are inseparable.",
        "- Investigate with read-only commands before changing anything.",
        "- Never start anything that takes the screen over or does not return on its own:",
        "  no editors, no pagers, no top, no -f/--follow. There is no way back out of them.",
        "- sudo can prompt for a password, which only the user can type. Prefer a command",
        "  that does not need root; if a step really does, use \"ask\" and let them decide.",
        "- Always pass non-interactive flags (-y, DEBIAN_FRONTEND=noninteractive).",
        "- Never use placeholders such as <path>. Read the value first if you do not know it.",
        "- Never ask permission to look at something. Read-only commands run without a prompt,",
        "  so just run them. Investigating is your job, not a decision for the user.",
        "",
        "What is blocked and what is merely confirmed:",
        "- Only a few catastrophic shapes are blocked outright: wiping the filesystem root,",
        "  formatting a disk, writing a raw device, shutting the machine down, killing every",
        "  process. Everything else that changes the system is simply confirmed by the user.",
        "- So a blocked command means \"narrow it down\", not \"give up\". Killing one named",
        "  process, restarting one service, editing one file are all allowed, with a prompt.",
        "",
        "When a step fails:",
        "- A non-zero exit is part of the task, not the end of it. Diagnose it and keep going.",
        "- Read the logs, config, and state around the failure before trying a different command.",
        "- If a program crashes or a service will not start, check its log file, its config, and",
        "  whether the port, file or permission it needs is free. Verify the fix by re-running it.",
        "- Never answer with \"done\" just to report an error you have not investigated.",
        "",
        "Finishing:",
        "- \"done\" means the thing the user asked for has been carried out and verified.",
        "  Reporting what you found is not the same as doing it. If the user asked you to",
        "  restart something, it is not done until it is running again and you have checked.",
        "- Use \"done\" if you have genuinely run out of options, saying what you tried and what blocks it.",
        "- Use \"ask\" only for a decision that is the user's to make, or a fact you cannot read",
        "  from the machine. Never use it to ask whether you may proceed with the task.",
        "",
    ]);
    if let Some(memory) = input.memory {
        out.extend(memory_section(memory));
    }
    if has_skills {
        out.extend(skill_section(input.skills));
    }
    out.push("Machine and session context:".to_string());
    out.push(input.context.to_string());
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base<'a>() -> PromptInput<'a> {
        PromptInput {
            language: Language::EnUs,
            context: "CWD: /home/me",
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
        assert!(text.ends_with("Do not answer with \"done\" merely to report this failure."));
    }

    #[test]
    fn a_clean_result_is_three_lines_and_no_lecture() {
        let result = CommandResult { output: "ok".into(), ..Default::default() };
        assert_eq!(describe_result(&result), "EXIT: 0\nOUTPUT:\nok");
    }

    #[test]
    fn empty_output_is_said_rather_than_left_blank() {
        let result = CommandResult::default();
        assert_eq!(describe_result(&result), "EXIT: 0\nOUTPUT:\n(no output)");
        let result = CommandResult { timed_out: true, ..Default::default() };
        assert!(describe_result(&result).contains("OUTPUT:\n(nothing)"));
    }

    #[test]
    fn a_truncated_result_says_where_the_middle_went() {
        let result =
            CommandResult { output: "x".into(), truncated: true, ..Default::default() };
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
        let text = describe_memory(MemoryOpKind::Remember, MemoryScope::Global, MemoryOutcome::Ok);
        assert_eq!(text, "Stored in global memory. Carry on with what you were doing.");
        let text = describe_memory(MemoryOpKind::Forget, MemoryScope::Server, MemoryOutcome::Ok);
        assert!(text.starts_with("Removed from this server's memory."));
        let text = describe_memory(MemoryOpKind::Remember, MemoryScope::Server, MemoryOutcome::Full);
        assert!(text.starts_with("This server's memory is full and NOTHING was written."));
        let text =
            describe_memory(MemoryOpKind::Remember, MemoryScope::Server, MemoryOutcome::Failed);
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
        let prompt = build_system_prompt(&PromptInput { memory: Some(&memory), ..base() });
        assert!(prompt.contains("{\"action\":\"remember\""));
        assert!(prompt.contains("[global]\n- prefers tabs"));
        assert!(prompt.contains("[server: web-1]\n- nginx is at /opt/nginx"));
    }

    #[test]
    fn empty_memory_still_invites_the_first_fact() {
        let memory = MemoryPrompt::default();
        let prompt = build_system_prompt(&PromptInput { memory: Some(&memory), ..base() });
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
        let prompt = build_system_prompt(&PromptInput { skills: &skills, ..base() });
        assert!(prompt.contains("{\"action\":\"skill\""));
        assert!(prompt.contains("- deploy: how releases go out here"));
    }

    #[test]
    fn the_reply_language_follows_the_setting() {
        let prompt = build_system_prompt(&base());
        assert!(prompt.contains("\"summary\" in English."));
        let prompt = build_system_prompt(&PromptInput { language: Language::ZhCn, ..base() });
        assert!(prompt.contains("\"summary\" in Chinese."));
        assert!(prompt.contains("- Reason in Chinese too."));
    }

    #[test]
    fn not_thinking_drops_the_instruction_about_thinking() {
        let prompt = build_system_prompt(&PromptInput { thinking: false, ..base() });
        assert!(!prompt.contains("Reason in English too."));
    }

    #[test]
    fn the_context_is_the_last_thing_the_model_reads() {
        let prompt = build_system_prompt(&base());
        assert!(prompt.ends_with("Machine and session context:\nCWD: /home/me"));
    }

    #[test]
    fn resolved_local_directories_are_listed_when_there_are_any() {
        let places = vec!["  Desktop: C:\\Users\\me\\Desktop".to_string()];
        let prompt = build_system_prompt(&PromptInput { local_places: &places, ..base() });
        assert!(prompt.contains("- Directories on that computer, resolved for you:"));
        assert!(prompt.contains("  Desktop: C:\\Users\\me\\Desktop"));
        let prompt = build_system_prompt(&base());
        assert!(!prompt.contains("- Directories on that computer, resolved for you:"));
    }
}
