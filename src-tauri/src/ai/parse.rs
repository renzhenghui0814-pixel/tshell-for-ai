//! Pulls the action out of a reply.
//!
//! A reply is prose, and an action is one JSON object at the very end of it.
//! Only that trailing object is read: JSON anywhere else in a reply is the model
//! showing the user an example, and reading it would run something nobody asked
//! for. That rule is what lets the assistant answer "what does this webhook body
//! look like?" without the answer being mistaken for an instruction.
//!
//! Nothing here strips fences. A fence around the trailing object is stepped
//! over by the scan; stripping them used to come first, which also emptied the
//! fences the model had written *inside* its prose -- so every snippet the
//! assistant showed the user arrived unmarked and the panel had nothing left to
//! highlight.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::types::{ActionKind, AgentAction, MemoryScope};

/// What a reply turned out to hold.
///
/// `unreadable` is the part that used to go unrecorded. A trailing object
/// carrying an `"action"` key that will not parse is not noise to step over: it
/// is an action the model believes it sent, and treating it as prose would
/// report the sentence before it as the answer to a task still waiting.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedReply {
    pub actions: Vec<AgentAction>,
    /// True when the reply ended in something action-shaped that would not parse.
    pub unreadable: bool,
    /// What the model wrote, with any trailing action object taken off the end.
    ///
    /// Models narrate. "I'll run a few read-only commands and tidy the result
    /// into a table" arrives ahead of the object most of the time, and every word
    /// of it used to be dropped on the floor: the loop took the action and had
    /// nowhere to put the sentence. What the user saw was a command card
    /// appearing out of nowhere, which is most of why the assistant could seem to
    /// work in silence.
    pub prose: String,
}

/// The verb of a candidate that will not parse, read off the text it was written
/// in. Matched rather than parsed precisely because the object around it is
/// broken; the value is a short bare word and arrives whole.
static ACTION_VERB: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#""action"\s*:\s*"([A-Za-z]+)""#).unwrap());
/// The fence a model puts around its object, matched at the end of a slice: once
/// after the object to find it, and once after the prose to tidy the line that
/// opened the fence.
static TRAILING_FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"```[A-Za-z]*\s*$").unwrap());
static NEWLINES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\r\n]+").unwrap());

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
                && chars[index + 2..index + 6]
                    .iter()
                    .all(|c| c.is_ascii_hexdigit())
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
///
/// Public because the tool-calling track needs the same repair: constrained
/// decoding makes a mangled argument list rare rather than impossible, and a
/// second repair written beside this one would drift from it.
pub fn read_object(candidate: &str) -> Option<serde_json::Map<String, Value>> {
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

/// The fields a verb cannot be carried out without.
///
/// Checked here rather than left to the step that would run it, because this is
/// also what tells an action apart from JSON the model was showing the user:
/// `{"action":"run"}` in the middle of an explanation names a verb but asks for
/// nothing, and carrying it out would be acting on an example.
///
/// `why` and `scope` are required of the model and deliberately not required
/// here. The parser has a safe answer for both -- an empty reason on the card,
/// the current server -- so refusing an otherwise complete action over one of
/// them would spend a round trip to gain a word.
///
/// `content`, `old` and `new` are checked for presence rather than for content:
/// an empty string is a file truncated to nothing and a fragment deleted, both
/// of which are things to ask for.
fn is_complete(action: &AgentAction, kind: ActionKind) -> bool {
    let given = |field: &Option<String>| field.as_ref().is_some_and(|text| !text.trim().is_empty());
    match kind {
        ActionKind::Run => given(&action.command),
        ActionKind::Write | ActionKind::Append => given(&action.path) && action.content.is_some(),
        ActionKind::Edit => {
            given(&action.path) && action.old_text.is_some() && action.new_text.is_some()
        }
        ActionKind::Download => !action.paths.is_empty(),
        // An upload naming nothing opens the file picker, which is the point of
        // it: the user chooses on their own machine, where the model cannot look.
        ActionKind::Upload => true,
        ActionKind::Remember => given(&action.text),
        ActionKind::Skill => given(&action.name),
    }
}

/// The fields of one action, however the object was arrived at.
///
/// Split out so a tool call and a JSON object become the same `AgentAction` by
/// the same rules. The tool track puts the verb in under `action` and hands the
/// arguments straight here -- which is what makes the two tracks agree about
/// every alias and every coercion below rather than only about most of them.
pub fn action_from_object(record: serde_json::Map<String, Value>) -> Option<AgentAction> {
    let verb = match record.get("action") {
        Some(Value::String(text)) => text.trim().to_lowercase(),
        _ => String::new(),
    };
    let kind = ActionKind::parse(&verb)?;

    let action = AgentAction {
        action: Some(kind),
        // A command stays one line: the marker protocol it is typed into is built
        // on that, which is exactly why the file actions exist instead.
        command: text_of(record.get("command"))
            .map(|command| NEWLINES.replace_all(&command, " ").trim().to_string()),
        why: text_of(record.get("why")),
        text: text_of(record.get("text")),
        path: text_of(record.get("path")),
        content: raw_of(record.get("content")),
        old_text: raw_of(record.get("old")).or_else(|| raw_of(record.get("oldText"))),
        new_text: raw_of(record.get("new")).or_else(|| raw_of(record.get("newText"))),
        // Anything but a plain "global" is the current server. Filing a fact too
        // narrowly costs one machine a repeated lookup; filing it too widely tells
        // the model something untrue about every other machine the user owns.
        scope: Some(
            match text_of(record.get("scope"))
                .map(|s| s.to_lowercase())
                .as_deref()
            {
                Some("global") => MemoryScope::Global,
                _ => MemoryScope::Server,
            },
        ),
        // A transfer takes one item or several, because moving three logs is one
        // intention and should not cost three round trips to the model.
        paths: path_list(record.get("path")),
        to: text_of(record.get("to")),
        // `skill` is accepted alongside `name`, because a model that has just been
        // told the action is called "skill" reaches for that key about as often as
        // it reaches for the right one.
        name: text_of(record.get("name")).or_else(|| text_of(record.get("skill"))),
        file: text_of(record.get("file")),
    };

    is_complete(&action, kind).then_some(action)
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

/// Every place the object at the end of the reply could begin, nearest last.
///
/// A start qualifies when its object closes exactly at the end of the reply, or
/// never closes at all. The second case is a reply that ran out of room inside
/// its own action: the half that arrived is not an answer to show anyone, and
/// the task is waiting on something that was never finished being written.
///
/// Several can qualify at once, because an unbalanced brace in the prose --
/// `awk '{print $1` in a sentence -- borrows the action's closing brace and
/// looks like a start too. So the caller tries them from the end backwards and
/// takes the first that reads: the nearest one is the object, and the ones
/// before it are the sentence it was written after.
///
/// The trailing fence is off before this is called, so an object is "at the end"
/// whether or not the model wrapped it in ```json.
fn trailing_starts(chars: &[char]) -> Vec<usize> {
    (0..chars.len())
        .filter(|start| chars[*start] == '{')
        .filter(|start| !matches!(balanced_end(chars, *start), Some(end) if end < chars.len()))
        .collect()
}

/// True when a candidate that will not parse was still trying to be an action.
///
/// The verb is the evidence, and the only evidence there is. A broken object
/// naming `run` is an instruction that lost its escaping, and the task is
/// waiting on it. A broken object naming `create` is the tail of an answer about
/// somebody else's API -- there are a lot of those -- and treating it as a lost
/// instruction costs the user their answer and two requests to not get it.
fn meant_to_act(candidate: &str) -> bool {
    ACTION_VERB
        .captures(candidate)
        .and_then(|found| ActionKind::parse(&found[1].to_lowercase()))
        .is_some()
}

/// What the model wrote and, if it ended in one, the action it asked for.
///
/// Only the trailing object is considered. JSON earlier in the reply is the
/// model showing the user something -- a config, a request body, the protocol
/// itself -- and reading it would carry out an example.
pub fn parse_actions(reply: &str) -> ParsedReply {
    let body = TRAILING_FENCE.replace(reply.trim_end(), "");
    let chars: Vec<char> = body.trim_end().chars().collect();

    // The line that opened the fence belongs to the object, not to the sentence
    // before it, so it goes with the object rather than being shown as speech.
    let prose_before = |start: usize| {
        let head: String = chars[..start].iter().collect();
        TRAILING_FENCE
            .replace(head.trim_end(), "")
            .trim()
            .to_string()
    };
    let prose_only = || ParsedReply {
        actions: Vec::new(),
        unreadable: false,
        prose: reply.trim().to_string(),
    };

    let starts = trailing_starts(&chars);
    let mut wreck = None;
    for start in starts.into_iter().rev() {
        let candidate: String = chars[start..].iter().collect();
        match read_object(&candidate) {
            // It reads. Whether it is an action is then a question about its verb
            // and its fields, and anything failing that is JSON the model was
            // quoting rather than sending.
            Some(record) => {
                if let Some(action) = action_from_object(record) {
                    return ParsedReply {
                        actions: vec![action],
                        unreadable: false,
                        prose: prose_before(start),
                    };
                }
            }
            // Kept in case nothing further out reads either. Only the outermost
            // wreck is reported, because that is the whole of what was lost.
            None if meant_to_act(&candidate) => wreck = Some(start),
            None => {}
        }
    }

    match wreck {
        Some(start) => ParsedReply {
            actions: Vec::new(),
            unreadable: true,
            prose: prose_before(start),
        },
        None => prose_only(),
    }
}

/// True when a piece of text ends in an action.
///
/// For the one caller that has text but no reply: a model that wrote its action
/// into the reasoning field instead of the answer.
pub fn carries_action(text: &str) -> bool {
    let parsed = parse_actions(text);
    !parsed.actions.is_empty() || parsed.unreadable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(reply: &str) -> AgentAction {
        let parsed = parse_actions(reply);
        assert_eq!(
            parsed.actions.len(),
            1,
            "expected exactly one action in {reply:?}"
        );
        parsed.actions.into_iter().next().unwrap()
    }

    fn none(reply: &str) -> ParsedReply {
        let parsed = parse_actions(reply);
        assert!(parsed.actions.is_empty(), "expected no action in {reply:?}");
        parsed
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
        let parsed =
            parse_actions("Let me look.\n```json\n{\"action\":\"run\",\"command\":\"ls\"}\n```");
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.prose, "Let me look.");
    }

    #[test]
    fn narration_before_the_object_is_kept() {
        let parsed =
            parse_actions("I'll check the disk first.\n{\"action\":\"run\",\"command\":\"df -h\"}");
        assert_eq!(parsed.prose, "I'll check the disk first.");
    }

    #[test]
    fn a_reply_that_is_all_prose_is_the_answer() {
        let parsed = none("The disk is nearly full. You should clear /var/log.");
        assert!(!parsed.unreadable);
        assert_eq!(
            parsed.prose,
            "The disk is nearly full. You should clear /var/log."
        );
    }

    #[test]
    fn brace_shaped_prose_before_the_action_does_not_swallow_it() {
        let parsed = parse_actions(
            "Use awk '{print $1}' for that.\n{\"action\":\"run\",\"command\":\"ls\"}",
        );
        assert_eq!(parsed.actions.len(), 1);
        assert_eq!(parsed.prose, "Use awk '{print $1}' for that.");
    }

    /// The whole point of reading only the trailing object. An answer that shows
    /// the user a request body is an answer, not an instruction -- and the older
    /// rule ran it.
    #[test]
    fn an_example_inside_the_answer_is_not_an_action() {
        let parsed = none(
            "You would post this:\n```json\n{\"action\":\"run\",\"command\":\"rm -rf /\"}\n```\n\
             and the server replies with 202.",
        );
        assert!(!parsed.unreadable);
        assert!(parsed.prose.contains("202"));
    }

    /// The commonest false alarm there was: any API with an "action" field. It
    /// used to leave the reply carrying wreckage, which cost the user the answer
    /// and two requests to not get it.
    #[test]
    fn a_verb_this_build_does_not_have_is_not_an_action() {
        let parsed = none(r#"Send {"action":"create","user":"bob"} to the endpoint."#);
        assert!(!parsed.unreadable);
        let broken = none(r#"Roughly: {"action": "create", "user": ...}"#);
        assert!(!broken.unreadable);
    }

    /// An object that names a verb but asks for nothing is an example of the
    /// protocol, not a use of it.
    #[test]
    fn an_action_missing_the_field_its_verb_needs_is_not_an_action() {
        none(r#"The shape is {"action":"run","why":"..."}"#);
        none(r#"{"action":"write","path":"/tmp/x"}"#);
        none(r#"{"action":"edit","path":"/tmp/x","old":"a"}"#);
        none(r#"{"action":"skill"}"#);
    }

    /// An empty file and a deleted fragment are things to ask for, so presence is
    /// what is checked and not length.
    #[test]
    fn an_empty_content_is_still_a_file_being_written() {
        let written = one(r#"{"action":"write","path":"/tmp/x","content":""}"#);
        assert_eq!(written.content.as_deref(), Some(""));
        let edited = one(r#"{"action":"edit","path":"/x","old":"a","new":""}"#);
        assert_eq!(edited.new_text.as_deref(), Some(""));
    }

    /// Naming nothing is how the user is shown a file picker.
    #[test]
    fn an_upload_needs_no_path_but_a_download_does() {
        assert!(one(r#"{"action":"upload","to":"/tmp","why":"x"}"#)
            .paths
            .is_empty());
        none(r#"{"action":"download","to":"/tmp","why":"x"}"#);
    }

    #[test]
    fn a_real_line_break_inside_a_string_is_mended() {
        let action =
            one("{\"action\":\"write\",\"path\":\"/a\",\"content\":\"line one\nline two\"}");
        assert_eq!(action.content.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn a_lone_backslash_is_taken_literally() {
        let action = one(r#"{"action":"run","command":"grep \d /tmp/x"}"#);
        assert_eq!(action.command.as_deref(), Some(r"grep \d /tmp/x"));
    }

    #[test]
    fn a_valid_unicode_escape_survives_the_repair() {
        let action = one(r#"{"action":"remember","text":"你好"}"#);
        assert_eq!(action.text.as_deref(), Some("你好"));
    }

    /// A broken object naming a verb this build has is an instruction that lost
    /// its escaping, and the task is waiting on it.
    #[test]
    fn an_unmendable_action_is_counted_rather_than_stepped_over() {
        let parsed = none("{\"action\":\"run\",\"command\":\"echo \"hi\" loudly\"}");
        assert!(parsed.unreadable);
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
        assert_eq!(
            one(r#"{"action":"remember","text":"x"}"#).scope,
            Some(MemoryScope::Server)
        );
        assert_eq!(
            one(r#"{"action":"remember","text":"x","scope":"everywhere"}"#).scope,
            Some(MemoryScope::Server)
        );
    }

    #[test]
    fn a_transfer_takes_one_path_or_several() {
        assert_eq!(
            one(r#"{"action":"upload","path":"a.txt","to":"/tmp"}"#).paths,
            vec!["a.txt"]
        );
        assert_eq!(
            one(r#"{"action":"download","path":["a","b"," "],"to":"/tmp"}"#).paths,
            vec!["a", "b"]
        );
    }

    #[test]
    fn the_skill_key_is_accepted_alongside_name() {
        assert_eq!(
            one(r#"{"action":"skill","skill":"deploy"}"#)
                .name
                .as_deref(),
            Some("deploy")
        );
        assert_eq!(
            one(r#"{"action":"skill","name":"deploy","skill":"other"}"#)
                .name
                .as_deref(),
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

    /// The outermost object, not the innermost: both close in the same place.
    #[test]
    fn an_object_holding_an_object_is_read_whole() {
        let action = one(r#"{"action":"run","command":"x","meta":{"a":1}}"#);
        assert_eq!(action.command.as_deref(), Some("x"));
    }

    /// A `{` the model never closed borrows the action's closing brace and looks
    /// like the start of one. The nearest start is the object; the ones before it
    /// are the sentence it was written after.
    #[test]
    fn an_unclosed_brace_in_the_prose_does_not_swallow_the_action() {
        let action = one("Use awk '{print $1 for that.
{\"action\":\"run\",\"command\":\"ls\"}");
        assert_eq!(action.command.as_deref(), Some("ls"));
    }

    /// An action inside a container is an example of one. Only the object the
    /// reply itself ends in is an instruction.
    #[test]
    fn an_action_nested_in_another_object_is_not_an_instruction() {
        let parsed = none(r#"The body is {"payload":{"action":"run","command":"rm -rf /"}}"#);
        assert!(!parsed.unreadable);
    }

    /// A reply that ran out of room stops inside its own action. Half an
    /// instruction is not an answer, and showing it as one would end a task that
    /// is still waiting.
    #[test]
    fn a_reply_cut_off_inside_its_action_is_not_shown_as_an_answer() {
        let parsed = none(
            "I will look.
{\"action\":\"run\",\"command\":\"tail -n 200 /var/log/mes",
        );
        assert!(parsed.unreadable);
        assert_eq!(parsed.prose, "I will look.");
    }

    #[test]
    fn an_unbalanced_object_closes_nothing() {
        let chars: Vec<char> = "{\"a\":1".chars().collect();
        assert_eq!(balanced_end(&chars, 0), None);
        let chars: Vec<char> = "{\"a\":\"}\"}".chars().collect();
        assert_eq!(balanced_end(&chars, 0), Some(9));
    }

    #[test]
    fn reasoning_that_carries_an_action_is_told_apart_from_reasoning_that_does_not() {
        assert!(carries_action(
            r#"I should look. {"action":"run","command":"ls"}"#
        ));
        assert!(!carries_action("I should look at the disk next."));
    }
}
