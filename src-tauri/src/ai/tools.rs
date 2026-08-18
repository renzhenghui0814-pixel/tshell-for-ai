//! The same eight verbs, described so the endpoint can constrain them.
//!
//! # Why there are two tracks and not one
//!
//! The protocol has always been "reply with exactly one JSON object". That works
//! everywhere and it fails in one particular way: the model writes the object
//! into prose, or puts a real line break inside a string, or writes `C:\logs`
//! with the backslash it meant -- and `parse.rs` is five hundred lines of reading
//! around that. Native tool calling removes the failure rather than repairing it,
//! because the endpoint constrains the decoding and what comes back is already an
//! object.
//!
//! It is not available everywhere. "OpenAI-compatible" is a claim about the
//! route, and a gateway fronting a model with no tool support rejects `tools`
//! with a 400. So both tracks are kept, and the prompt keeps describing the JSON
//! protocol whether or not tools went out: an endpoint that refuses the field
//! gets the request again without it -- see [`ExtraField::Tools`] in `llm/http.rs`
//! -- and the model then answers the old way, into a parser that never went away.
//!
//! [`ExtraField::Tools`]: super::llm::ExtraField::Tools
//!
//! # One definition of the protocol, not two
//!
//! Every tool here is named for its verb and takes the verb's own argument names.
//! That is what lets a call become an action by putting `action` back into the
//! arguments and handing the result to [`super::parse::action_from_object`] --
//! the same function the JSON track ends in. Aliases, coercions, the rule that a
//! transfer path may be one string or a list: all of it is written once and both
//! tracks get it. A second reader here would drift from the first, and it would
//! drift silently, in the direction of running something the user did not mean.

use serde_json::{json, Map, Value};

use super::llm::{ToolCall, ToolSpec};
use super::parse::{action_from_object, read_object};
use super::types::AgentAction;

/// A required string argument, with the wording the prompt uses for it.
fn text(description: &str) -> Value {
    json!({ "type": "string", "description": description })
}

fn schema(properties: Vec<(&str, Value)>, required: &[&str]) -> Value {
    let mut map = Map::new();
    for (name, shape) in properties {
        map.insert(name.to_string(), shape);
    }
    json!({
        "type": "object",
        "properties": Value::Object(map),
        "required": required,
        // Endpoints that support it will refuse an argument nobody declared,
        // which is one fewer way for a hallucinated field to reach the loop
        // looking like a real one.
        "additionalProperties": false,
    })
}

fn tool(name: &str, description: &str, parameters: Value) -> ToolSpec {
    ToolSpec { name: name.into(), description: description.into(), parameters }
}

/// The reason shown to the user on the confirmation card. Every acting verb has
/// one, and it is required, because a card that cannot say why is a card nobody
/// can answer.
fn why() -> (&'static str, Value) {
    ("why", text("Why this is needed, in one short clause."))
}

/// What the model may call this task.
///
/// The two flags are the prompt's own: an action the model is never shown is one
/// it never tries and is never refused for, and switching memory off by not
/// mentioning it is cleaner than switching it off by answering it.
pub fn tool_specs(has_memory: bool, has_skills: bool) -> Vec<ToolSpec> {
    let mut tools = vec![
        tool(
            "run",
            "Run one shell command in the user's own terminal, on the remote machine.",
            schema(
                vec![
                    ("command", text("A single-line shell command.")),
                    why(),
                ],
                &["command", "why"],
            ),
        ),
        tool(
            "write",
            "Create a file, or replace one entirely, with the content given here.",
            schema(
                vec![
                    ("path", text("Absolute path on the remote machine.")),
                    ("content", text("The entire file, exactly as it should end up.")),
                    why(),
                ],
                &["path", "content", "why"],
            ),
        ),
        tool(
            "append",
            "Add text to the end of a file, leaving what is already there alone.",
            schema(
                vec![
                    ("path", text("Absolute path on the remote machine.")),
                    ("content", text("The text to add.")),
                    why(),
                ],
                &["path", "content", "why"],
            ),
        ),
        tool(
            "edit",
            "Replace one exact fragment of a file. The fragment must occur exactly once.",
            schema(
                vec![
                    ("path", text("Absolute path on the remote machine.")),
                    ("old", text("The exact existing text to replace, occurring exactly once.")),
                    ("new", text("What replaces it.")),
                    why(),
                ],
                &["path", "old", "new", "why"],
            ),
        ),
        tool(
            "download",
            "Copy files from the remote machine to the user's own computer.",
            schema(
                vec![
                    ("path", paths("Remote path, or a list of remote paths.")),
                    ("to", text("Folder on the user's computer to put them in.")),
                    why(),
                ],
                &["path", "why"],
            ),
        ),
        tool(
            "upload",
            "Copy files from the user's own computer to the remote machine.",
            schema(
                vec![
                    ("path", paths("Local path, or a list of local paths.")),
                    ("to", text("Folder on the remote machine to put them in.")),
                    why(),
                ],
                &["why"],
            ),
        ),
    ];

    if has_memory {
        tools.push(tool(
            "remember",
            "File one durable fact, so it is known at the start of later conversations.",
            schema(
                vec![
                    scope(),
                    ("text", text("One durable fact, in a single line.")),
                    why(),
                ],
                &["scope", "text", "why"],
            ),
        ));
    }

    if has_skills {
        tools.push(tool(
            "skill",
            "Read one of the user's skills, for instructions on how they want something done.",
            schema(
                vec![
                    ("name", text("The skill's name, as listed in the system prompt.")),
                    ("file", text("A file inside the skill. Omit for its main file.")),
                    why(),
                ],
                &["name", "why"],
            ),
        ));
    }

    tools
}

/// A transfer's `path`, which is one string or several.
///
/// Declared as both because moving three logs is one intention and should not
/// cost three round trips to the model. `parse.rs` flattens either shape.
fn paths(description: &str) -> Value {
    json!({
        "anyOf": [
            { "type": "string" },
            { "type": "array", "items": { "type": "string" } }
        ],
        "description": description,
    })
}

fn scope() -> (&'static str, Value) {
    (
        "scope",
        json!({
            "type": "string",
            "enum": ["server", "global"],
            "description": "\"server\" for this machine alone, \"global\" for every machine.",
        }),
    )
}

/// What the model called, as an action this loop can carry out.
///
/// `None` for a name this build does not have -- a model calling a tool nobody
/// offered -- and for arguments that will not parse even after repair. Both reach
/// the loop as "this reply carried nothing", which is the same place a mangled
/// JSON object arrives at, and takes the same nudge.
pub fn action_from_call(call: &ToolCall) -> Option<AgentAction> {
    let arguments = call.arguments.trim();
    // A tool with every argument optional is legitimately called with nothing at
    // all, and endpoints spell that as `""`, `"{}"` or a missing field.
    let mut record = if arguments.is_empty() {
        Map::new()
    } else {
        read_object(arguments)?
    };
    // The verb travels as the tool's name rather than as an argument, so it is
    // put back before the shared reader sees the object. Overwritten rather than
    // filled in: what the endpoint says was called outranks anything the model
    // may have written into the arguments.
    record.insert("action".into(), Value::String(call.name.trim().to_lowercase()));
    action_from_object(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{ActionKind, MemoryScope};

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall { id: "call_1".into(), name: name.into(), arguments: arguments.into() }
    }

    #[test]
    fn a_call_becomes_the_action_of_the_same_name() {
        let action = action_from_call(&call(
            "run",
            r#"{"command":"df -h","why":"check disk"}"#,
        ))
        .unwrap();
        assert_eq!(action.action, Some(ActionKind::Run));
        assert_eq!(action.command.as_deref(), Some("df -h"));
        assert_eq!(action.why.as_deref(), Some("check disk"));
    }

    /// The whole reason the verb is not an argument: what the endpoint says was
    /// called is what was called.
    #[test]
    fn the_tool_name_outranks_an_action_written_into_the_arguments() {
        let action = action_from_call(&call(
            "remember",
            r#"{"action":"run","command":"rm -rf /","text":"hi"}"#,
        ))
        .unwrap();
        assert_eq!(action.action, Some(ActionKind::Remember));
    }

    #[test]
    fn a_tool_this_build_does_not_have_is_no_action_at_all() {
        assert!(action_from_call(&call("sudo", r#"{"command":"x"}"#)).is_none());
    }

    /// Endpoints spell "no arguments" three different ways, and `upload` is the
    /// one verb that means something without any: the user picks the files.
    #[test]
    fn a_call_with_no_arguments_is_still_a_call() {
        for spelling in ["", "{}", "   "] {
            let action = action_from_call(&call("upload", spelling)).unwrap();
            assert_eq!(action.action, Some(ActionKind::Upload));
        }
    }

    /// The same completeness rule the JSON track applies. A call that names a
    /// verb and asks for nothing is not a step, whichever track it arrived on.
    #[test]
    fn a_call_missing_the_field_its_verb_needs_is_no_action() {
        assert!(action_from_call(&call("run", r#"{"why":"look around"}"#)).is_none());
        assert!(action_from_call(&call("write", r#"{"path":"/tmp/x","why":"x"}"#)).is_none());
    }

    /// Constrained decoding makes this rare, not impossible -- and when it does
    /// happen it is the same fault the JSON track sees, so it takes the same
    /// repair rather than a second one written beside it.
    #[test]
    fn arguments_are_mended_the_way_a_json_reply_is() {
        let action = action_from_call(&call(
            "write",
            "{\"path\":\"/tmp/a\",\"content\":\"line one\nline two\",\"why\":\"x\"}",
        ))
        .unwrap();
        assert_eq!(action.content.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn a_transfer_takes_one_path_or_several() {
        let one = action_from_call(&call(
            "download",
            r#"{"path":"/var/log/a.log","to":"/tmp","why":"x"}"#,
        ))
        .unwrap();
        assert_eq!(one.paths, vec!["/var/log/a.log".to_string()]);

        let many = action_from_call(&call(
            "download",
            r#"{"path":["/a","/b"],"to":"/tmp","why":"x"}"#,
        ))
        .unwrap();
        assert_eq!(many.paths, vec!["/a".to_string(), "/b".to_string()]);
    }

    /// Filing a fact too widely tells the model something untrue about every
    /// other machine the user owns, so anything but a plain "global" narrows.
    #[test]
    fn a_memory_scope_that_is_not_global_is_this_machine() {
        let action =
            action_from_call(&call("remember", r#"{"scope":"nonsense","text":"a","why":"b"}"#))
                .unwrap();
        assert_eq!(action.scope, Some(MemoryScope::Server));
    }

    #[test]
    fn memory_and_skills_are_only_offered_when_they_are_on() {
        let names = |memory, skills| {
            tool_specs(memory, skills)
                .into_iter()
                .map(|tool| tool.name)
                .collect::<Vec<_>>()
        };

        let bare = names(false, false);
        assert!(!bare.iter().any(|name| name == "remember"));
        assert!(!bare.iter().any(|name| name == "skill"));

        let full = names(true, true);
        assert!(full.iter().any(|name| name == "remember"));
        assert!(full.iter().any(|name| name == "skill"));
    }

    /// Nothing here ends the task. A reply that calls no tool is the answer, on
    /// both tracks, and a verb for saying so would be a second way to speak --
    /// one that arrives beside the prose the model already wrote and has to be
    /// drawn instead of it.
    #[test]
    fn no_tool_speaks_to_the_user_or_ends_the_task() {
        for spec in tool_specs(true, true) {
            assert!(
                !matches!(spec.name.as_str(), "say" | "ask" | "done"),
                "{} is offered but the task ends in prose",
                spec.name
            );
        }
    }

    /// Every tool has to be one the loop can dispatch. A name here that
    /// `ActionKind` does not know would be offered to the model, called, and then
    /// silently dropped as unreadable.
    #[test]
    fn every_tool_offered_is_a_verb_the_loop_carries_out() {
        for spec in tool_specs(true, true) {
            assert!(
                ActionKind::parse(&spec.name).is_some(),
                "{} is offered but is not an action",
                spec.name
            );
            assert_eq!(spec.parameters["type"], "object");
        }
    }

    /// The schema and the parser have to want the same things.
    ///
    /// The endpoint enforces `required` for the tool track and nothing enforces
    /// it for the JSON track, so the parser carries its own rule -- and two rules
    /// about the same protocol drift. `why` and `scope` are the deliberate gap:
    /// required of the model, defaulted by the parser rather than refused over.
    #[test]
    fn every_required_argument_is_one_the_parser_insists_on() {
        for spec in tool_specs(true, true) {
            let required: Vec<String> = spec.parameters["required"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|name| name.as_str())
                .filter(|name| !matches!(*name, "why" | "scope"))
                .map(str::to_string)
                .collect();
            for missing in &required {
                let mut arguments = Map::new();
                for name in &required {
                    if name != missing {
                        arguments.insert(name.clone(), Value::String("x".into()));
                    }
                }
                let call = ToolCall {
                    id: "call_1".into(),
                    name: spec.name.clone(),
                    arguments: Value::Object(arguments).to_string(),
                };
                assert!(
                    action_from_call(&call).is_none(),
                    "{} without {missing} was accepted as an action",
                    spec.name
                );
            }
        }
    }
}

