//! Pulls the user-facing prose out of a reply while it is still arriving.
//!
//! The protocol asks for exactly one JSON object, which is the wrong shape for
//! streaming: nothing can be parsed until the closing brace, so the whole answer
//! lands at once however slowly it was produced. So the object is watched rather
//! than parsed, and the characters of the one field meant for the user are handed
//! over as they decode -- unescaped, because `\n` is a line break to the reader
//! and a pair of characters only to the wire.
//!
//! Only "say", "done" and "ask" carry such a field. Until the action is known the
//! decoded characters are held back, because "remember" also has a "text" and
//! pouring a memory line into the thread as though it were an answer would be a
//! lie about what just happened. The action is the first key the protocol asks
//! for, so in practice the hold lasts a dozen characters.
//!
//! A reply that is not JSON at all is the other half of the job, and it used to
//! be missed entirely. The loop treats plain prose as a first-class answer -- a
//! question that wanted an answer rather than a command gets one -- but this had
//! no state for it, so every character of such a reply failed both tests above
//! and was swallowed.
//!
//! Telling the two apart cannot be done from the first character, because a model
//! that is about to send an action often narrates first: "I'll run a few commands
//! and tidy the result into a table", then the object. Painting that and wiping it
//! a moment later when the command card opens is worse than not painting it. So
//! the opening is held for [`HOLD_CHARS`] and the decision is made on whether a
//! `{` turned up in it.
//!
//! Nothing here decides anything: the reply is still parsed properly afterwards,
//! and this only ever governs what was painted early.

use std::sync::LazyLock;

use regex::Regex;

/// `"action"` and its value. Matched rather than parsed because the value is a
/// short bare word that arrives whole; a partial match simply waits for more.
static ACTION_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""action"\s*:\s*"([A-Za-z]+)""#).unwrap());
/// The opening quote of the field being streamed. A backslash cannot precede the
/// key's own quotes, so an occurrence of `\"text\":\"` inside some other string
/// -- a file being written, say -- does not match this.
static SPOKEN_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""(?:text|summary|question)"\s*:\s*""#).unwrap());

/// How much of the opening is held before prose is committed to.
///
/// Measured against real replies rather than guessed. A narrated action puts its
/// brace within about thirty characters -- the sentence before it is one clause,
/// because the model believes it is sending an object and is only clearing its
/// throat. A prose answer opens with a heading or a paragraph and runs for
/// thousands of characters before a brace, if one ever comes.
///
/// Erring long costs a fraction of a second of held text at the start of an
/// answer. Erring short costs a sentence painted and then wiped by a command
/// card, which is the thing that made this worth fixing in the first place.
const HOLD_CHARS: usize = 120;

/// What a reply turned out to be. `Undecided` is the opening hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Undecided,
    Json,
    Prose,
}

/// Which actions have something in them the user is meant to read.
fn is_spoken(action: &str) -> bool {
    matches!(action.to_lowercase().as_str(), "say" | "done" | "ask")
}

#[derive(Debug, Default)]
pub struct SayStreamer {
    raw: Vec<char>,
    /// Where the decoder has read up to. Only meaningful once `open` is true.
    cursor: usize,
    open: bool,
    closed: bool,
    /// Decoded characters waiting for the action to say whether they may be shown.
    pending: String,
    allowed: Option<bool>,
    shape: Option<Shape>,
}

impl SayStreamer {
    pub fn new() -> Self {
        Self { shape: Some(Shape::Undecided), ..Default::default() }
    }

    /// True when the field being read never met its closing quote.
    ///
    /// The difference between a reply that stopped and a reply that broke, and
    /// the only reliable way to tell them apart after the fact. A model that
    /// finished its sentence and forgot the closing `"}` leaves the string open
    /// to the last character; a model that put a bare `"` in the middle of a
    /// shell command closed it early and carried on writing past it. The first is
    /// an answer worth keeping. The second is a fragment, and showing it would
    /// present the first few words of a broken command as though they were what
    /// the user asked for.
    pub fn unterminated(&self) -> bool {
        self.open && !self.closed
    }

    /// Returns the text to append to the thread, which is usually most of `chunk`.
    pub fn push(&mut self, chunk: &str) -> String {
        if self.closed && self.allowed.is_some() {
            return String::new();
        }
        self.raw.extend(chunk.chars());

        // A brace anywhere in the opening means an object is on its way, whether
        // it came first or after a sentence of narration, and the JSON path below
        // is the one that knows what to do with it. Its absence, once enough has
        // arrived to be sure, means there is no object coming and the whole reply
        // is the answer.
        if self.shape == Some(Shape::Undecided) {
            if self.raw.contains(&'{') {
                self.shape = Some(Shape::Json);
            } else if self.raw.len() >= HOLD_CHARS {
                self.shape = Some(Shape::Prose);
            }
        }
        if self.shape == Some(Shape::Prose) {
            // Handed over exactly as it arrived: this is text, not a JSON string,
            // so there is nothing to unescape and a backslash in it is a backslash.
            let out: String = self.raw[self.cursor..].iter().collect();
            self.cursor = self.raw.len();
            return out;
        }
        if self.shape == Some(Shape::Undecided) {
            return String::new();
        }

        let text: String = self.raw.iter().collect();
        if self.allowed.is_none() {
            if let Some(found) = ACTION_KEY.captures(&text) {
                self.allowed = Some(is_spoken(&found[1]));
            }
        }

        if !self.open {
            let Some(key) = SPOKEN_KEY.find(&text) else {
                return String::new();
            };
            self.open = true;
            // Byte offsets from the regex, turned back into the char index this
            // scanner counts in.
            self.cursor = text[..key.end()].chars().count();
        }

        let decoded = self.drain();
        match self.allowed {
            Some(false) => {
                self.pending.clear();
                String::new()
            }
            None => {
                self.pending.push_str(&decoded);
                String::new()
            }
            Some(true) => {
                let mut out = std::mem::take(&mut self.pending);
                out.push_str(&decoded);
                out
            }
        }
    }

    /// Decodes as far as the buffer allows, stopping on the closing quote or on a
    /// truncated escape. A chunk boundary can land inside an escape, and half of
    /// one is not a character.
    fn drain(&mut self) -> String {
        let mut out = String::new();
        let mut at = self.cursor;

        while at < self.raw.len() {
            let char = self.raw[at];
            if char == '"' {
                self.closed = true;
                break;
            }
            if char != '\\' {
                out.push(char);
                at += 1;
                continue;
            }

            let Some(escape) = self.raw.get(at + 1).copied() else { break };
            if escape == 'u' {
                if self.raw.len() < at + 6 {
                    break;
                }
                let digits: String = self.raw[at + 2..at + 6].iter().collect();
                match u32::from_str_radix(&digits, 16).ok().and_then(char::from_u32) {
                    Some(decoded) => out.push(decoded),
                    // A lone surrogate, or four characters that are not hex. Left
                    // as it was written rather than guessed at.
                    None => out.push_str(&format!("\\u{digits}")),
                }
                at += 6;
                continue;
            }
            out.push(match escape {
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\u{8}',
                'f' => '\u{c}',
                other => other,
            });
            at += 2;
        }

        self.cursor = at;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_say_is_streamed_as_it_decodes() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"say","text":"hel"#), "hel");
        assert_eq!(streamer.push("lo there"), "lo there");
        assert_eq!(streamer.push(r#""}"#), "");
    }

    #[test]
    fn escapes_are_unescaped_on_the_way_out() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"say","text":"a\nb\tc\"d"#), "a\nb\tc\"d");
    }

    #[test]
    fn an_escape_split_across_chunks_waits_for_its_other_half() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"say","text":"a\"#), "a");
        assert_eq!(streamer.push("nb"), "\nb");
    }

    #[test]
    fn a_unicode_escape_split_across_chunks_waits_too() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"say","text":"\u4f"#), "");
        assert_eq!(streamer.push("60"), "你");
    }

    #[test]
    fn a_memory_line_is_never_poured_into_the_thread() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"remember","text":"nginx is at /opt""#), "");
    }

    #[test]
    fn text_before_the_action_is_known_is_held_and_then_released() {
        let mut streamer = SayStreamer::new();
        // The spoken key arrives before the action key, so the decoded characters
        // wait rather than being shown or dropped.
        assert_eq!(streamer.push(r#"{"text":"hello"#), "");
        assert_eq!(streamer.push(r#"","action":"say""#), "hello");
    }

    #[test]
    fn a_prose_reply_streams_once_the_hold_is_over() {
        let mut streamer = SayStreamer::new();
        let opening = "x".repeat(HOLD_CHARS - 1);
        assert_eq!(streamer.push(&opening), "");
        assert_eq!(streamer.push("yz"), format!("{opening}yz"));
        assert_eq!(streamer.push(" more"), " more");
    }

    #[test]
    fn narration_before_an_object_is_not_painted() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push("I'll check the disk first.\n"), "");
        assert_eq!(streamer.push(r#"{"action":"run","command":"df -h"}"#), "");
    }

    #[test]
    fn an_unterminated_field_is_told_from_a_closed_one() {
        let mut streamer = SayStreamer::new();
        streamer.push(r#"{"action":"say","text":"half a sentence"#);
        assert!(streamer.unterminated());

        let mut streamer = SayStreamer::new();
        streamer.push(r#"{"action":"say","text":"whole"}"#);
        assert!(!streamer.unterminated());
    }

    #[test]
    fn a_summary_and_a_question_are_spoken_too() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"done","summary":"all set"#), "all set");

        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"ask","question":"which one?"#), "which one?");
    }

    #[test]
    fn a_whole_answer_pushed_at_once_still_comes_back() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"say","text":"complete"}"#), "complete");
    }

    #[test]
    fn wide_characters_survive_a_chunk_boundary_mid_string() {
        let mut streamer = SayStreamer::new();
        assert_eq!(streamer.push(r#"{"action":"say","text":"你"#), "你");
        assert_eq!(streamer.push("好"), "好");
    }
}
