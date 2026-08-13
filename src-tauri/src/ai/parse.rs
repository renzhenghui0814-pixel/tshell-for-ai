//! Pulls the action out of a reply.
//!
//! Models wrap JSON in prose and code fences even when told not to, so the first
//! balanced object wins rather than requiring the whole reply to parse.
//!
//! A fence around the JSON needs no removing: the scan below looks for a
//! balanced object and steps over anything else. Stripping them used to come
//! first, which also emptied the fences the model had written *inside* "text" --
//! so every snippet the assistant showed the user arrived as unmarked prose, and
//! the panel had nothing left to highlight.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::types::{ActionKind, AgentAction, MemoryScope};

/// What a reply turned out to hold.
///
/// `unreadable` is the part that used to go unrecorded. A candidate carrying an
/// `"action"` key that will not parse is not noise to step over: it is an action
/// the model believes it sent. Stepping over it silently ran the *next* one
/// instead -- so a reply of "say this, then run that" whose say had a real line
/// break in it ran the command and never showed the sentence, while the model
/// went on believing the user had read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedReply {
    pub actions: Vec<AgentAction>,
    /// Action-shaped candidates that would not parse, repaired or not.
    pub unreadable: u32,
    /// True when the first action-shaped thing in the reply was one of those.
    pub leading: bool,
    /// Whatever the model wrote before the action, with any code fence stripped.
    ///
    /// Models narrate. "I'll run a few read-only commands and tidy the result
    /// into a table" arrives ahead of the object perhaps half the time, and every
    /// word of it used to be dropped on the floor: the loop took the action and
    /// had nowhere to put the sentence. What the user saw was a command card
    /// appearing out of nowhere, which is most of why the assistant could seem to
    /// work in silence.
    pub preamble: String,
}

/// The key that marks a candidate as something that meant to be an action.
static ACTION_KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""action"\s*:"#).unwrap());
static TRAILING_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"```[A-Za-z]*\s*$").unwrap());
static FENCE_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*```[A-Za-z]*[ \t]*$\r?\n?").unwrap());
static NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n]+").unwrap());

/// True when nothing in the reply was ever trying to be an action.
///
/// A model that mangled its JSON leaves the wreck behind: an `"action"` key sat
/// in something that would not parse, or named a verb this client does not have.
/// A model that simply answered leaves no trace of one -- and brace-shaped text
/// is no evidence either way, because the CSS in a design plan balances exactly
/// as well as an action does. So the key is what is looked for, not the braces.
pub fn is_prose(reply: &str) -> bool {
    !ACTION_KEY.is_match(reply)
}

/// A fence and the whitespace around it are punctuation, not something said.
///
/// Stripped rather than shown because a model that wraps its object in ```json
/// writes the opening fence before it, which would otherwise arrive as a line of
/// the assistant's own speech reading "```json".
fn clean_preamble(text: &str) -> String {
    let text = TRAILING_FENCE.replace(text, "");
    FENCE_LINE.replace_all(&text, "").trim().to_string()
}

/// The escapes JSON actually has. `u` is left out: its digits are checked instead.
const VALID_ESCAPES: &[char] = &['"', '\\', '/', 'b', 'f', 'n', 'r', 't'];

/// A second attempt at a candidate the parser refused, or `None` when there was
/// nothing to mend.
///
/// Two faults account for almost every unparseable reply, and the prompt already
/// warns about both -- which is the tell that warning is not enough. A real line
/// break inside a string, because the model laid its answer out the way it will
/// be read. And a lone backslash, because `\d`, `\1` and `C:\logs` are written
/// the way they are meant rather than the way JSON needs them.
///
/// Both are unambiguous to repair: a raw control character inside a JSON string
/// is illegal, so escaping it cannot change the meaning of anything legal, and a
/// backslash that opens no escape JSON recognises was a literal backslash. What
/// is deliberately NOT attempted is a guess -- an unescaped quote mid-string ends
/// the string as far as any scanner can tell, and rewriting on a hunch would turn
/// a reply the model can be asked to send again into a command nobody wrote.
fn repair_json(candidate: &str) -> Option<String> {
    let chars: Vec<char> = candidate.chars().collect();
    let mut out = String::new();
    let mut in_string = false;
    let mut mended = false;
    let mut index = 0;

    while index < chars.len() {
        let char = chars[index];
        if !in_string {
            if char == '"' {
                in_string = true;
            }
            out.push(char);
            index += 1;
            continue;
        }
        if char == '"' {
            in_string = false;
            out.push(char);
            index += 1;
            continue;
        }
        if char == '\\' {
            let next = chars.get(index + 1).copied();
            if next == Some('u')
                && chars.len() >= index + 6
                && chars[index + 2..index + 6].iter().all(|c| c.is_ascii_hexdigit())
            {
                out.extend(&chars[index..index + 6]);
                index += 6;
                continue;
            }
            if next.is_some_and(|next| VALID_ESCAPES.contains(&next)) {
                out.push(char);
                out.push(next.unwrap());
                index += 2;
                continue;
            }
            out.push_str("\\\\");
            mended = true;
            index += 1;
            continue;
        }
        if (char as u32) < 0x20 {
            match char {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{8}' => out.push_str("\\b"),
                '\u{c}' => out.push_str("\\f"),
                other => out.push_str(&format!("\\u{:04x}", other as u32)),
            }
            mended = true;
            index += 1;
            continue;
        }
        out.push(char);
        index += 1;
    }
    if mended {
        Some(out)
    } else {
        None
    }
}

/// One candidate as an object, tried straight and then mended.
fn read_object(candidate: &str) -> Option<serde_json::Map<String, Value>> {
    let mended = repair_json(candidate);
    for text in [Some(candidate.to_string()), mended].into_iter().flatten() {
        // The straight read failing is the normal case here; the mended one
        // failing means the reply was broken in a way nothing may guess at.
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) {
            return Some(map);
        }
    }
    None
}

fn text_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.trim().to_string()),
        _ => None,
    }
}

/// File content is data and is taken exactly as it was sent. Trimming it would
/// drop the leading blank line of a config file and the indentation of the first
/// line of a script, and for `old` it would stop the fragment matching at all.
fn raw_of(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// `path` on a transfer is one string or an array of them; both arrive here as a list.
fn path_list(value: Option<&Value>) -> Vec<String> {
    let items: Vec<&Value> = match value {
        Some(Value::Array(items)) => items.iter().collect(),
        Some(other) => vec![other],
        None => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| item.as_str())
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// One candidate, or `None` when it does not parse or is not an action.
pub fn read_action(candidate: &str) -> Option<AgentAction> {
    let record = read_object(candidate)?;

    let verb = match record.get("action") {
        Some(Value::String(text)) => text.trim().to_lowercase(),
        _ => String::new(),
    };
    let action = ActionKind::parse(&verb)?;

    Some(AgentAction {
        action: Some(action),
        // A command stays one line: the marker protocol it is typed into is built
        // on that, which is exactly why the file actions exist instead.
        command: text_of(record.get("command"))
            .map(|command| NEWLINES.replace_all(&command, " ").trim().to_string()),
        why: text_of(record.get("why")),
        question: text_of(record.get("question")),
        summary: text_of(record.get("summary")),
        text: text_of(record.get("text")),
        path: text_of(record.get("path")),
        content: raw_of(record.get("content")),
        old_text: raw_of(record.get("old")).or_else(|| raw_of(record.get("oldText"))),
        new_text: raw_of(record.get("new")).or_else(|| raw_of(record.get("newText"))),
        // Anything but a plain "global" is the current server. Filing a fact too
        // narrowly costs one machine a repeated lookup; filing it too widely tells
        // the model something untrue about every other machine the user owns.
        scope: Some(match text_of(record.get("scope")).map(|s| s.to_lowercase()).as_deref() {
            Some("global") => MemoryScope::Global,
            _ => MemoryScope::Server,
        }),
        // A transfer takes one item or several, because moving three logs is one
        // intention and should not cost three round trips to the model.
        paths: path_list(record.get("path")),
        to: text_of(record.get("to")),
        // `skill` is accepted alongside `name`, because a model that has just been
        // told the action is called "skill" reaches for that key about as often as
        // it reaches for the right one.
        name: text_of(record.get("name")).or_else(|| text_of(record.get("skill"))),
        file: text_of(record.get("file")),
    })
}

/// Where the object opening at `start` closes, or `None`. Braces in strings do
/// not count.
pub fn balanced_end(chars: &[char], start: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for index in start..chars.len() {
        let char = chars[index];
        if escaped {
            escaped = false;
            continue;
        }
        if char == '\\' {
            escaped = true;
            continue;
        }
        if char == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if char == '{' {
            depth += 1;
        } else if char == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(index + 1);
            }
        }
    }
    None
}

/// Every `{...}` that balances, in the order they appear.
///
/// The first one used to be the only one tried, which threw away the answer
/// whenever anything brace-shaped came before it -- an `awk '{print $1}'` quoted
/// in a sentence, a snippet of JSON being discussed rather than sent. The step
/// then ended in a protocol complaint over a reply that had the action in it all
/// along. Trying the rest costs nothing: a reply holds a handful of braces.
///
/// Spans rather than slices, so the caller can tell a candidate that follows
/// another from one that sits inside it. Only the first distinction matters: the
/// braces inside a `say` belong to the sentence it is showing the user, and are
/// not a second action that the reader was cheated out of.
fn json_objects(chars: &[char]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    for start in 0..chars.len() {
        if chars[start] != '{' {
            continue;
        }
        if let Some(end) = balanced_end(chars, start) {
            found.push((start, end));
        }
    }
    found
}

/// Every action in a reply, in the order they were written.
///
/// The loop takes one step at a time and runs the first. The rest matter anyway:
/// a model that sent two edits believes both happened, so it has to be told they
/// did not rather than left to find out from a compiler.
///
/// Candidates nested inside one that was already read are skipped rather than
/// counted. A `say` may legitimately quote the protocol at the user, and the
/// braces inside its own text are not a second action that went missing.
pub fn parse_actions(reply: &str) -> ParsedReply {
    let chars: Vec<char> = reply.chars().collect();
    let mut actions = Vec::new();
    let mut unreadable = 0;
    let mut leading = false;
    let mut covered = 0;
    // Where the first thing that meant to be an action began.
    let mut first_at = chars.len();

    for (start, end) in json_objects(&chars) {
        if start < covered {
            continue;
        }
        let candidate: String = chars[start..end].iter().collect();
        if let Some(action) = read_action(&candidate) {
            if actions.is_empty() && unreadable == 0 {
                first_at = start;
            }
            actions.push(action);
            covered = end;
            continue;
        }
        if !ACTION_KEY.is_match(&candidate) {
            continue;
        }
        if unreadable == 0 && actions.is_empty() {
            leading = true;
            first_at = start;
        }
        unreadable += 1;
        covered = end;
    }

    // Only what came before the protocol started. A reply that is entirely prose
    // has no action to precede, and the loop shows the whole of it as the answer.
    let preamble = if !actions.is_empty() || unreadable > 0 {
        clean_preamble(&chars[..first_at].iter().collect::<String>())
    } else {
        String::new()
    };
    ParsedReply { actions, unreadable, leading, preamble }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(reply: &str) -> AgentAction {
        let parsed = parse_actions(reply);
        assert_eq!(parsed.actions.len(), 1, "expected exactly one action in {reply:?}");
        parsed.actions.into_iter().next().unwrap()
    }

    #[test]
    fn a_bare_object_is_read() {
        let action = one(r#"{"action":"run","command":"ls -la","why":"look"}"#);
        assert_eq!(action.action, Some(ActionKind::Run));
        assert_eq!(action.command.as_deref(), Some("ls -la"));
        assert_eq!(action.why.as_deref(), Some("look"));
    }

    #[test]
    fn a_fenced_object_needs_no_unwrapping() {
        let parsed = parse_actions("Let me look.\n```json\n{\"action\":\"run\",\"command\":\"ls\"}\n```");
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.preamble, "Let me look.");
    }

    #[test]
    fn narration_before_the_object_is_kept() {
        let parsed = parse_actions("I'll check the disk first.\n{\"action\":\"run\",\"command\":\"df -h\"}");
        assert_eq!(parsed.preamble, "I'll check the disk first.");
    }

    #[test]
    fn a_reply_that_is_all_prose_has_no_preamble_to_take() {
        let parsed = parse_actions("The disk is nearly full. You should clear /var/log.");
        assert!(parsed.actions.is_empty());
        assert_eq!(parsed.unreadable, 0);
        assert_eq!(parsed.preamble, "");
        assert!(is_prose("The disk is nearly full."));
    }

    #[test]
    fn brace_shaped_prose_before_the_action_does_not_swallow_it() {
        let parsed = parse_actions("Use awk '{print $1}' for that.\n{\"action\":\"done\",\"summary\":\"ok\"}");
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.actions[0].action, Some(ActionKind::Done));
    }

    #[test]
    fn braces_inside_a_say_are_not_a_second_action() {
        let parsed = parse_actions(r#"{"action":"say","text":"send {\"action\":\"run\"} to run one"}"#);
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.actions[0].action, Some(ActionKind::Say));
    }

    #[test]
    fn every_action_in_a_reply_is_reported() {
        let parsed = parse_actions(
            r#"{"action":"say","text":"one"} then {"action":"run","command":"ls"}"#,
        );
        assert_eq!(parsed.actions.len(), 2);
        assert_eq!(parsed.actions[1].action, Some(ActionKind::Run));
    }

    #[test]
    fn a_real_line_break_inside_a_string_is_mended() {
        let action = one("{\"action\":\"say\",\"text\":\"line one\nline two\"}");
        assert_eq!(action.text.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn a_lone_backslash_is_taken_literally() {
        let action = one(r#"{"action":"run","command":"grep \d /tmp/x"}"#);
        assert_eq!(action.command.as_deref(), Some(r"grep \d /tmp/x"));
    }

    #[test]
    fn a_valid_unicode_escape_survives_the_repair() {
        let action = one(r#"{"action":"say","text":"\u4f60\u597d"}"#);
        assert_eq!(action.text.as_deref(), Some("你好"));
    }

    #[test]
    fn an_unmendable_action_is_counted_rather_than_stepped_over() {
        let parsed = parse_actions("{\"action\":\"say\",\"text\":\"he said \"hi\" loudly\"}");
        assert!(parsed.actions.is_empty());
        assert_eq!(parsed.unreadable, 1);
        assert!(parsed.leading);
    }

    #[test]
    fn an_unknown_verb_is_not_an_action_but_is_still_wreckage() {
        let parsed = parse_actions(r#"{"action":"teleport","to":"mars"}"#);
        assert!(parsed.actions.is_empty());
        assert_eq!(parsed.unreadable, 1);
    }

    #[test]
    fn a_command_is_flattened_to_one_line() {
        let action = one("{\"action\":\"run\",\"command\":\"ls \\n -la\"}");
        assert_eq!(action.command.as_deref(), Some("ls   -la"));
    }

    #[test]
    fn file_content_is_taken_byte_for_byte() {
        let action = one(r#"{"action":"write","path":"/tmp/x","content":"\n  indented\n"}"#);
        assert_eq!(action.content.as_deref(), Some("\n  indented\n"));
    }

    #[test]
    fn scope_narrows_to_the_server_unless_it_says_global() {
        assert_eq!(
            one(r#"{"action":"remember","text":"x","scope":"global"}"#).scope,
            Some(MemoryScope::Global)
        );
        assert_eq!(
            one(r#"{"action":"remember","text":"x","scope":"GLOBAL"}"#).scope,
            Some(MemoryScope::Global)
        );
        assert_eq!(one(r#"{"action":"remember","text":"x"}"#).scope, Some(MemoryScope::Server));
        assert_eq!(
            one(r#"{"action":"remember","text":"x","scope":"everywhere"}"#).scope,
            Some(MemoryScope::Server)
        );
    }

    #[test]
    fn a_transfer_takes_one_path_or_several() {
        assert_eq!(one(r#"{"action":"upload","path":"a.txt","to":"/tmp"}"#).paths, vec!["a.txt"]);
        assert_eq!(
            one(r#"{"action":"download","path":["a","b"," "],"to":"/tmp"}"#).paths,
            vec!["a", "b"]
        );
    }

    #[test]
    fn the_skill_key_is_accepted_alongside_name() {
        assert_eq!(one(r#"{"action":"skill","skill":"deploy"}"#).name.as_deref(), Some("deploy"));
        assert_eq!(
            one(r#"{"action":"skill","name":"deploy","skill":"other"}"#).name.as_deref(),
            Some("deploy")
        );
    }

    #[test]
    fn old_and_new_are_read_under_either_spelling() {
        let action = one(r#"{"action":"edit","path":"/x","old":"a","new":"b"}"#);
        assert_eq!(action.old_text.as_deref(), Some("a"));
        assert_eq!(action.new_text.as_deref(), Some("b"));
        let action = one(r#"{"action":"edit","path":"/x","oldText":"a","newText":"b"}"#);
        assert_eq!(action.old_text.as_deref(), Some("a"));
    }

    #[test]
    fn a_wreck_before_a_good_action_marks_the_reply_as_leading() {
        let parsed = parse_actions(
            "{\"action\":\"say\",\"text\":\"he said \"hi\"\"} {\"action\":\"run\",\"command\":\"ls\"}",
        );
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.unreadable, 1);
        assert!(parsed.leading);
    }

    #[test]
    fn an_unbalanced_object_closes_nothing() {
        let chars: Vec<char> = "{\"a\":1".chars().collect();
        assert_eq!(balanced_end(&chars, 0), None);
        let chars: Vec<char> = "{\"a\":\"}\"}".chars().collect();
        assert_eq!(balanced_end(&chars, 0), Some(9));
    }
}
