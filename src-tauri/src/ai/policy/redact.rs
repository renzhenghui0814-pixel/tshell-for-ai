//! Cleaning applied to anything read off the terminal before a model sees it.
//!
//! Kept in its own module because both the transcript and the agent's own command
//! output go through it, and neither should have to depend on the other.

use std::sync::LazyLock;

use regex::Regex;

/// CSI and OSC sequences, charset selects, and the stray control bytes around them.
static ANSI: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        &[
            r"\x1b\[[0-9;?]*[ -/]*[@-~]",
            r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)",
            r"\x1b[()][0-9A-Za-z]",
            r"\x1b[=><]",
            r"[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]",
        ]
        .join("|"),
    )
    .unwrap()
});

static SECRET_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b([A-Za-z0-9_-]*(?:password|passwd|pwd|token|secret|api[_-]?key|access[_-]?key|credential)[A-Za-z0-9_-]*)(\s*[:=]\s*)(\S+)",
    )
    .unwrap()
});
static BEARER_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\bauthorization\s*:\s*)(\S.*)").unwrap());
static PRIVATE_KEY_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN [^-\n]*PRIVATE KEY-----(?s:.*?)-----END [^-\n]*PRIVATE KEY-----")
        .unwrap()
});

pub fn strip_ansi(text: &str) -> String {
    ANSI.replace_all(text, "").into_owned()
}

/// Best-effort masking of anything that looks like a credential.
pub fn redact(text: &str) -> String {
    let text = PRIVATE_KEY_BLOCK.replace_all(text, "[REDACTED PRIVATE KEY]");
    let text = BEARER_HEADER.replace_all(&text, "${1}[REDACTED]");
    SECRET_KEY.replace_all(&text, "${1}${2}[REDACTED]").into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_sequences_leave_nothing_behind() {
        assert_eq!(strip_ansi("\x1b[2mdim\x1b[0m"), "dim");
        assert_eq!(strip_ansi("\x1b]97;s1234\x07body"), "body");
        assert_eq!(strip_ansi("a\x1b(Bb"), "ab");
        assert_eq!(strip_ansi("a\x00b\x7fc"), "abc");
    }

    #[test]
    fn newlines_and_tabs_survive_because_they_are_the_text() {
        assert_eq!(strip_ansi("a\nb\tc\r"), "a\nb\tc\r");
    }

    #[test]
    fn anything_named_like_a_secret_is_masked() {
        assert_eq!(redact("PASSWORD=hunter2"), "PASSWORD=[REDACTED]");
        assert_eq!(redact("api_key: abc123"), "api_key: [REDACTED]");
        assert_eq!(redact("MY_ACCESS_KEY = zzz"), "MY_ACCESS_KEY = [REDACTED]");
        assert_eq!(redact("Authorization: Bearer xyz"), "Authorization: [REDACTED]");
    }

    #[test]
    fn a_private_key_goes_whole() {
        let text = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\ndef\n-----END OPENSSH PRIVATE KEY-----";
        assert_eq!(redact(text), "[REDACTED PRIVATE KEY]");
    }

    #[test]
    fn ordinary_prose_is_left_alone() {
        assert_eq!(redact("the token bucket is full"), "the token bucket is full");
        assert_eq!(redact("total 48"), "total 48");
    }
}
