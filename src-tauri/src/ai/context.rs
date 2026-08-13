//! Turns a live terminal session into the short text block the model sees.
//!
//! Everything here is derived from the transcript `shell.rs` keeps, so nothing
//! extra is read from the remote machine. Commands are recovered from the echoed
//! prompt lines rather than from keystrokes on purpose: what a shell never echoes
//! (a sudo password) can then never reach the model.

use std::sync::LazyLock;

use regex::Regex;

use super::policy::redact::{redact, strip_ansi};

const DEFAULT_BUDGET: usize = 4000;
const MAX_COMMANDS: usize = 10;

/// A prompt line looks like `[user@host dir]$ cmd` or `user@host:/dir# cmd`.
static PROMPT_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^.*[@:].*[$#]\s+(\S.*)$").unwrap());
/// The end of a prompt: the last character of the prompt itself, its terminator,
/// and the space after it. A terminator sits against the prompt, which is what
/// tells it apart from a `#` that starts the command.
static PROMPT_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\S[$#]\s+").unwrap());
/// A trailing fragment that is another prompt rather than a command.
static BARE_PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[@:].*[$#]$").unwrap());

#[derive(Debug, Clone, Default)]
pub struct MachineFacts {
    pub os: String,
    pub kernel: String,
    pub shell: String,
    pub user: String,
    pub home: String,
}

pub struct ContextInput<'a> {
    pub host: &'a str,
    pub machine: &'a MachineFacts,
    pub transcript: &'a str,
    /// What `pwd` answered before this task. Empty when the shell would not say.
    pub cwd: &'a str,
    pub send_output: bool,
    pub output_lines: usize,
    pub budget: usize,
}

/// The command on one prompt line, if there is one.
///
/// Taking everything after the last `$` or `#` is wrong when the command itself
/// begins with one: the `# tshell edit /etc/nginx.conf` a file step draws would
/// come back as `tshell edit /etc/nginx.conf`, which is not a comment about
/// something that happened but an invitation to run a program that does not
/// exist. So the boundary is the last terminator sitting directly against the
/// prompt. Prompts written with a space before the `$` have no such boundary and
/// fall back to the looser reading, which is what they had before.
fn command_on(line: &str) -> String {
    let Some(found) = PROMPT_LINE.captures(line) else { return String::new() };

    let mut boundary = None;
    let mut prompts = 0;
    for hit in PROMPT_END.find_iter(line) {
        boundary = Some(hit.end());
        prompts += 1;
    }

    let command = match boundary {
        Some(at) => line[at..].trim(),
        None => found[1].trim(),
    };
    /*
     * Two prompts on one line with nothing typed at the second: the tail is a
     * prompt, not a command. Nothing should collapse like that now, but a step
     * interrupted before it could erase its line still can. Only asked of a line
     * that did collapse, because a command can end in `$` too -- `grep x:y$` is
     * one -- and on an ordinary line what was typed is what was typed.
     */
    if prompts > 1 && BARE_PROMPT.is_match(command) {
        String::new()
    } else {
        command.to_string()
    }
}

/// The commands visible in a transcript, keeping only the last `limit`.
pub fn extract_commands(transcript: &str, limit: Option<usize>) -> Vec<String> {
    let stripped = strip_ansi(transcript);
    // The agent types into this same terminal, so its commands are here too. The
    // transcript holds what was displayed, which is the command without its
    // plumbing.
    let commands: Vec<String> = stripped
        .split('\n')
        .map(|line| command_on(line.trim_end_matches('\r').trim_end()))
        .filter(|command| !command.is_empty())
        .collect();
    match limit {
        None => commands,
        Some(limit) if commands.len() <= limit => commands,
        Some(limit) => commands[commands.len() - limit..].to_vec(),
    }
}

pub fn build_context(input: &ContextInput) -> String {
    let budget = if input.budget == 0 { DEFAULT_BUDGET } else { input.budget };
    let clean = redact(&strip_ansi(input.transcript));
    let recent = extract_commands(&clean, Some(MAX_COMMANDS));
    let facts = input.machine;

    let or_unknown = |value: &str| if value.is_empty() { "unknown" } else { value }.to_string();
    let mut lines = vec![
        format!("Host: {}", input.host),
        format!(
            "OS: {} | kernel {} | shell {} | user {} | home {}",
            or_unknown(&facts.os),
            or_unknown(&facts.kernel),
            or_unknown(&facts.shell),
            or_unknown(&facts.user),
            or_unknown(&facts.home)
        ),
        /*
         * Measured, not worked out.
         *
         * This used to be replayed from the `cd` commands still visible in the
         * transcript, which was wrong in more ways than it was right: a `cd`
         * inside `cd x && make` was read as a directory literally named
         * "x && make", one written `cd $HOME/x` was dropped without trace, and a
         * prompt that does not happen to contain an `@` or a `:` hid every
         * command in the session, pinning this to $HOME however far the user had
         * walked. The model was then told the result was where its shell had been
         * placed, so it built absolute paths on it and wrote files into
         * directories that did not exist.
         *
         * The shell can simply be asked. Nothing here is inferred any more, and
         * when the answer does not arrive the line says so rather than guessing.
         */
        if input.cwd.is_empty() {
            "Working directory: unknown -- run pwd before you build a path out of it".to_string()
        } else {
            format!("Working directory: {}", input.cwd)
        },
    ];
    if !recent.is_empty() {
        lines.push("Recent commands:".to_string());
        lines.extend(recent.iter().map(|command| format!("  {command}")));
    }

    let context = lines.join("\n");
    if !input.send_output {
        return context;
    }

    let all: Vec<&str> = clean
        .split('\n')
        .map(|line| line.trim_end_matches('\r').trim_end())
        .filter(|line| !line.is_empty())
        .collect();
    let tail = &all[all.len().saturating_sub(input.output_lines)..];
    let header = format!("Recent output (last {} lines):", tail.len());

    // Filled from the bottom up, because the newest line is the one worth keeping
    // when the budget only has room for some of them.
    let mut kept: Vec<&str> = Vec::new();
    for line in tail.iter().rev() {
        let mut candidate = vec![context.clone(), header.clone()];
        candidate.push(format!("  {line}"));
        candidate.extend(kept.iter().map(|kept| format!("  {kept}")));
        if candidate.join("\n").len() > budget {
            break;
        }
        kept.insert(0, line);
    }
    if kept.is_empty() {
        return context;
    }
    let mut out = vec![context, header];
    out.extend(kept.iter().map(|line| format!("  {line}")));
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> MachineFacts {
        MachineFacts {
            os: "Ubuntu 24.04".into(),
            kernel: "6.8.0".into(),
            shell: "/bin/bash".into(),
            user: "me".into(),
            home: "/home/me".into(),
        }
    }

    fn input<'a>(transcript: &'a str, machine: &'a MachineFacts) -> ContextInput<'a> {
        ContextInput {
            host: "web-1",
            machine,
            transcript,
            cwd: "/srv/app",
            send_output: false,
            output_lines: 40,
            budget: 0,
        }
    }

    #[test]
    fn a_command_is_read_off_the_prompt_line_that_echoed_it() {
        let transcript = "me@web-1:/srv$ ls -la\ntotal 0\nme@web-1:/srv$ df -h\n";
        assert_eq!(extract_commands(transcript, None), vec!["ls -la", "df -h"]);
    }

    #[test]
    fn only_the_last_few_commands_are_kept() {
        let transcript: String =
            (1..=20).map(|n| format!("me@h:/$ command{n}\n")).collect();
        let commands = extract_commands(&transcript, Some(3));
        assert_eq!(commands, vec!["command18", "command19", "command20"]);
    }

    #[test]
    fn a_command_that_starts_with_a_hash_survives_intact() {
        let transcript = "me@web-1:/srv$ # tshell edit /etc/nginx.conf\n";
        assert_eq!(extract_commands(transcript, None), vec!["# tshell edit /etc/nginx.conf"]);
    }

    #[test]
    fn a_collapsed_line_ending_in_a_bare_prompt_yields_no_command() {
        // Two prompt boundaries on one line and nothing typed at the last: the
        // tail is a prompt, not a command. A step interrupted before it could
        // erase its own line leaves exactly this behind.
        assert!(extract_commands("a@b:/$ ls a@b:/$ c@d:/$\n", None).is_empty());
    }

    #[test]
    fn a_command_may_itself_end_in_a_terminator() {
        let transcript = "me@web-1:/srv$ grep x:y$\n";
        assert_eq!(extract_commands(transcript, None), vec!["grep x:y$"]);
    }

    #[test]
    fn escapes_and_ordinary_output_are_not_commands() {
        let transcript = "\x1b[32mtotal 48\x1b[0m\ndrwxr-xr-x 2 me me\n";
        assert!(extract_commands(transcript, None).is_empty());
    }

    #[test]
    fn the_block_names_the_machine_and_where_the_shell_is_standing() {
        let machine = facts();
        let context = build_context(&input("me@web-1:/srv$ ls\n", &machine));
        assert!(context.starts_with("Host: web-1\n"));
        assert!(context.contains("OS: Ubuntu 24.04 | kernel 6.8.0 | shell /bin/bash"));
        assert!(context.contains("Working directory: /srv/app"));
        assert!(context.contains("Recent commands:\n  ls"));
    }

    #[test]
    fn an_unknown_directory_says_so_rather_than_guessing() {
        let machine = MachineFacts::default();
        let mut given = input("", &machine);
        given.cwd = "";
        let context = build_context(&given);
        assert!(context.contains("Working directory: unknown -- run pwd"));
        assert!(context.contains("OS: unknown | kernel unknown"));
        assert!(!context.contains("Recent commands:"));
    }

    #[test]
    fn output_is_only_sent_when_it_was_asked_for() {
        let machine = facts();
        let transcript = "me@h:/$ echo hi\nhi\n";
        let context = build_context(&input(transcript, &machine));
        assert!(!context.contains("Recent output"));

        let mut given = input(transcript, &machine);
        given.send_output = true;
        let context = build_context(&given);
        assert!(context.contains("Recent output (last 2 lines):"));
        assert!(context.contains("  hi"));
    }

    #[test]
    fn the_newest_output_is_what_survives_a_tight_budget() {
        let machine = facts();
        let transcript = "first line\nsecond line\nthird line\n";
        let mut given = input(transcript, &machine);
        given.send_output = true;
        // Room for the block, the header and two of the three lines.
        given.budget = 175;
        let context = build_context(&given);
        assert!(context.contains("third line"), "{context}");
        assert!(context.contains("second line"), "{context}");
        assert!(!context.contains("first line"), "{context}");
    }

    #[test]
    fn a_budget_with_no_room_at_all_drops_the_output_section() {
        let machine = facts();
        let mut given = input("some output\n", &machine);
        given.send_output = true;
        given.budget = 10;
        let context = build_context(&given);
        assert!(!context.contains("Recent output"));
    }

    #[test]
    fn anything_that_looks_like_a_credential_is_masked_on_the_way_out() {
        let machine = facts();
        let mut given = input("me@h:/$ env\nAPI_KEY=secret123\n", &machine);
        given.send_output = true;
        let context = build_context(&given);
        assert!(context.contains("API_KEY=[REDACTED]"));
        assert!(!context.contains("secret123"));
    }
}
