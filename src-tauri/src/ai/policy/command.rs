//! Decides whether a generated command may run on its own.
//!
//! The shape of this check matters more than its contents. A denylist answers "is
//! this one of the bad ones?" and runs everything it does not recognise; an
//! allowlist answers "is this one of the known-harmless ones?" and asks about
//! everything else. Only the second is safe to put behind automatic execution, so
//! an unfamiliar command always reaches the user.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::risk::{scan_risk, split_segments, RiskLevel, RiskReason, Segment};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Auto,
    Confirm,
    Refuse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyResult {
    pub verdict: Verdict,
    /// Populated for `refuse`, so the model can be told why and try again.
    pub reasons: Vec<RiskReason>,
    /// Populated for `confirm`, naming the first thing that disqualified it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocker: Option<String>,
}

impl PolicyResult {
    fn auto() -> Self {
        Self {
            verdict: Verdict::Auto,
            reasons: Vec::new(),
            blocker: None,
        }
    }
    fn confirm(blocker: &str) -> Self {
        Self {
            verdict: Verdict::Confirm,
            reasons: Vec::new(),
            blocker: Some(blocker.to_string()),
        }
    }
    fn refuse(reasons: Vec<RiskReason>) -> Self {
        Self {
            verdict: Verdict::Refuse,
            reasons,
            blocker: None,
        }
    }
}

/// Extra constraints for commands that are read-only only in some shapes.
struct ReadOnlyRule {
    /// When present, the first operand must be one of these.
    subcommands: Option<&'static [&'static str]>,
    /// Flags or operands that turn the command into a write.
    deny_tokens: Option<&'static [&'static str]>,
}

const BARE: ReadOnlyRule = ReadOnlyRule {
    subcommands: None,
    deny_tokens: None,
};

/// Commands that only ever read, in every shape they take.
const BARE_COMMANDS: &[&str] = &[
    "ls",
    "dir",
    "vdir",
    "cat",
    "tac",
    "nl",
    "head",
    "wc",
    "stat",
    "file",
    "readlink",
    "realpath",
    "dirname",
    "basename",
    "pwd",
    "echo",
    "printf",
    "du",
    "df",
    "free",
    "uptime",
    "uname",
    "hostname",
    "whoami",
    "id",
    "groups",
    "date",
    "env",
    "printenv",
    "which",
    "type",
    "whereis",
    "locale",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "ag",
    "sort",
    "uniq",
    "cut",
    "tr",
    "column",
    "diff",
    "cmp",
    "md5sum",
    "sha1sum",
    "sha256sum",
    "cksum",
    "ps",
    "pgrep",
    "lsof",
    "netstat",
    "ss",
    "dmesg",
    "lsblk",
    "lscpu",
    "lsusb",
    "zcat",
    "zgrep",
    "getent",
    "nproc",
    "arch",
    "dmidecode",
];

/// The ones that read only in some shapes, with the constraint that makes it so.
const CONSTRAINED: &[(&str, ReadOnlyRule)] = &[
    // Following a stream never returns, which would hang the step until it times out.
    (
        "tail",
        ReadOnlyRule {
            subcommands: None,
            deny_tokens: Some(&["-f", "-F", "--follow"]),
        },
    ),
    (
        "journalctl",
        ReadOnlyRule {
            subcommands: None,
            deny_tokens: Some(&["-f", "--follow"]),
        },
    ),
    // find can delete and execute, which is exactly what an allowlist must not wave through.
    (
        "find",
        ReadOnlyRule {
            subcommands: None,
            deny_tokens: Some(&[
                "-exec", "-execdir", "-ok", "-okdir", "-delete", "-fprint", "-fprintf",
            ]),
        },
    ),
    (
        "sed",
        ReadOnlyRule {
            subcommands: None,
            deny_tokens: Some(&["-i", "--in-place"]),
        },
    ),
    (
        "tar",
        ReadOnlyRule {
            subcommands: Some(&["-t", "--list", "-tf", "-tzf", "-tvf"]),
            deny_tokens: None,
        },
    ),
    (
        "ip",
        ReadOnlyRule {
            subcommands: Some(&["addr", "a", "route", "r", "link", "neigh", "n", "rule"]),
            deny_tokens: Some(&["set", "add", "del", "delete", "change", "replace", "flush"]),
        },
    ),
    (
        "systemctl",
        ReadOnlyRule {
            subcommands: Some(&[
                "status",
                "is-active",
                "is-enabled",
                "is-failed",
                "list-units",
                "list-unit-files",
                "list-timers",
                "show",
                "cat",
            ]),
            deny_tokens: None,
        },
    ),
    (
        "docker",
        ReadOnlyRule {
            subcommands: Some(&[
                "ps", "logs", "inspect", "images", "version", "info", "stats", "top",
            ]),
            deny_tokens: Some(&["-f", "--follow"]),
        },
    ),
    (
        "kubectl",
        ReadOnlyRule {
            subcommands: Some(&["get", "describe", "logs", "version", "top", "explain"]),
            deny_tokens: Some(&["-f", "--follow"]),
        },
    ),
    (
        "git",
        ReadOnlyRule {
            subcommands: Some(&[
                "status",
                "log",
                "diff",
                "show",
                "branch",
                "remote",
                "tag",
                "blame",
                "describe",
                "rev-parse",
            ]),
            deny_tokens: None,
        },
    ),
];

/// Interpreters and dispatchers that can run anything at all.
const ARBITRARY_EXECUTORS: &[&str] = &[
    "sh", "bash", "zsh", "ksh", "dash", "csh", "tcsh", "fish", "python", "python2", "python3",
    "perl", "ruby", "node", "php", "lua", "awk", "gawk", "mawk", "xargs", "tee", "eval", "exec",
    "source", "nc", "ncat", "curl", "wget", "ssh", "scp", "rsync", "ftp", "telnet",
];

/// Constructs that let a harmless-looking command do arbitrary work. Their
/// presence anywhere in the line forces a confirmation, whatever the command
/// names are.
static STRUCTURAL_BLOCKERS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    vec![
        ("redirect", Regex::new(r"(^|[^0-9<>&])>>?[^&]").unwrap()),
        ("redirect", Regex::new(r">\s*$").unwrap()),
        /*
         * Both streams into a file: `&> log`, `&>> log`, `>& log`. The rule above
         * cannot see these -- it excludes `&` on either side of the `>` so that a
         * plain `2>&1` stays automatic -- and until these two lines existed the
         * only thing stopping them was the splitter mistaking part of the
         * redirection for a command it had never heard of. That was an accident,
         * and accidents stop.
         *
         * The same operators followed by a digit (`2>&1`, `>&2`) point one stream
         * at another and write nothing, so the file case is told apart by what
         * follows.
         */
        ("redirect", Regex::new(r"&>>?\s*[^\s&>]").unwrap()),
        ("redirect", Regex::new(r">&\s*[^\s0-9]").unwrap()),
        ("commandSubstitution", Regex::new(r"\$\(|`").unwrap()),
        ("processSubstitution", Regex::new(r"<\(").unwrap()),
        (
            "privilege",
            Regex::new(r"(^|[\s|;&(])(sudo|doas|su)\b").unwrap(),
        ),
        ("background", Regex::new(r"&\s*$").unwrap()),
    ]
});

fn first_operand(args: &[String]) -> Option<&String> {
    args.iter().find(|arg| !arg.starts_with('-'))
}

fn rule_for(name: &str) -> Option<&'static ReadOnlyRule> {
    if BARE_COMMANDS.contains(&name) {
        return Some(&BARE);
    }
    CONSTRAINED
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, rule)| rule)
}

/// The reason this segment is not allowed through unasked, or `None` when it is.
fn allows_segment(segment: &Segment, extra: &HashSet<&str>) -> Option<String> {
    let name = match segment.name.rfind('/') {
        Some(at) => &segment.name[at + 1..],
        None => segment.name.as_str(),
    };
    if ARBITRARY_EXECUTORS.contains(&name) {
        return Some(format!("executor:{name}"));
    }

    let rule = match rule_for(name) {
        Some(rule) => rule,
        None if extra.contains(name) => &BARE,
        None => return Some(format!("unknown:{name}")),
    };

    if let Some(deny) = rule.deny_tokens {
        if segment.args.iter().any(|arg| deny.contains(&arg.as_str())) {
            return Some(format!("writes:{name}"));
        }
    }
    if let Some(subcommands) = rule.subcommands {
        let sub = first_operand(&segment.args).or_else(|| segment.args.first());
        match sub {
            Some(sub) if subcommands.contains(&sub.as_str()) => {}
            _ => return Some(format!("subcommand:{name}")),
        }
    }
    None
}

/// `extra_read_only` are additional command names the user has declared read-only
/// in the config file, for in-house query tools the built-in list cannot know.
pub fn classify_command(command: &str, extra_read_only: &[String]) -> PolicyResult {
    let text = command.trim();
    if text.is_empty() {
        return PolicyResult::confirm("empty");
    }

    let risk = scan_risk(text);
    if risk.level == RiskLevel::Danger {
        return PolicyResult::refuse(risk.reasons);
    }

    if let Some((token, _)) = STRUCTURAL_BLOCKERS
        .iter()
        .find(|(_, test)| test.is_match(text))
    {
        return PolicyResult::confirm(token);
    }

    let segments = split_segments(text);
    if segments.is_empty() {
        return PolicyResult::confirm("empty");
    }

    let extra: HashSet<&str> = extra_read_only
        .iter()
        .map(|name| name.trim())
        .filter(|name| !name.is_empty())
        .collect();
    for segment in &segments {
        if let Some(blocker) = allows_segment(segment, &extra) {
            return PolicyResult::confirm(&blocker);
        }
    }
    PolicyResult::auto()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict(command: &str) -> Verdict {
        classify_command(command, &[]).verdict
    }
    fn blocker(command: &str) -> String {
        classify_command(command, &[]).blocker.unwrap_or_default()
    }

    #[test]
    fn known_read_only_commands_run_unasked() {
        assert_eq!(verdict("ls -la /tmp"), Verdict::Auto);
        assert_eq!(verdict("cat /etc/hosts | grep localhost"), Verdict::Auto);
        assert_eq!(verdict("ls 2>&1"), Verdict::Auto);
    }

    #[test]
    fn an_unfamiliar_command_always_reaches_the_user() {
        assert_eq!(verdict("frobnicate --all"), Verdict::Confirm);
        assert_eq!(blocker("frobnicate --all"), "unknown:frobnicate");
    }

    #[test]
    fn the_user_can_widen_the_allowlist() {
        assert_eq!(
            classify_command("frobnicate --all", &["frobnicate".into()]).verdict,
            Verdict::Auto
        );
        assert_eq!(
            classify_command("frobnicate", &["  ".into()]).verdict,
            Verdict::Confirm
        );
    }

    #[test]
    fn a_read_only_command_in_a_writing_shape_is_confirmed() {
        assert_eq!(blocker("tail -f /var/log/syslog"), "writes:tail");
        assert_eq!(blocker("sed -i s/a/b/ x"), "writes:sed");
        assert_eq!(blocker("git push"), "subcommand:git");
        assert_eq!(verdict("git status"), Verdict::Auto);
    }

    #[test]
    fn interpreters_are_never_automatic() {
        assert_eq!(blocker("bash -c 'ls'"), "executor:bash");
        assert_eq!(blocker("/usr/bin/python3 x.py"), "executor:python3");
    }

    #[test]
    fn structure_alone_forces_a_confirmation() {
        assert_eq!(blocker("echo x > /tmp/y"), "redirect");
        assert_eq!(blocker("echo $(whoami)"), "commandSubstitution");
        assert_eq!(blocker("diff <(ls) <(ls)"), "processSubstitution");
        assert_eq!(blocker("sudo ls"), "privilege");
        assert_eq!(blocker("ls &"), "background");
        assert_eq!(blocker("ls &> out"), "redirect");
    }

    #[test]
    fn destructive_commands_are_refused_before_anything_else_is_asked() {
        let result = classify_command("sudo rm -rf /", &[]);
        assert_eq!(result.verdict, Verdict::Refuse);
        assert_eq!(result.reasons, vec![RiskReason::RemovesRoot]);
        assert!(result.blocker.is_none());
    }

    #[test]
    fn an_empty_command_is_not_automatic() {
        assert_eq!(blocker("   "), "empty");
    }
}
