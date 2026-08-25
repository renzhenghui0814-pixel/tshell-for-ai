//! Where conversations live between sessions.
//!
//! `messages` is the single source for both readers. User records carry the text,
//! images and task context once; assistant records carry prose and native calls;
//! tool records carry the structured facts needed to redraw a card and to form
//! the observation sent back to the model. Loading a page and restoring a model
//! session are therefore two projections of one sequence rather than two stored
//! transcripts that can drift apart.
//!
//! One file per conversation, next to the config file. They hold real command
//! output from real machines, so they are somewhere the user can find, read and
//! delete rather than inside an opaque key-value store.

use std::path::PathBuf;

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::ai::llm::{ChatImage, ChatMessage, ChatRole, ToolCall};
use crate::ai::policy::command::Verdict;
use crate::ai::prompt::describe_result;
use crate::ai::tools::action_from_call;
use crate::ai::types::{
    ActionKind, AgentAction, AgentEvent, CommandResult, FileEncoding, FileOpKind, TransferKind,
};
use crate::atomic;

/// Enough history to be useful, few enough that the list stays a list.
const MAX_LISTED: usize = 200;

/// The opening message, cut to something that fits a row in the list.
const MAX_TITLE: usize = 60;

/// Shorter than a list row: a tab is read at a glance and out of the corner of
/// an eye.
const MAX_TAB_TITLE: usize = 24;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatEntry {
    /// `user` or `event`.
    pub kind: String,
    /// For a user turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ChatImage>,
    /// For everything the assistant did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<AgentEvent>,
    /// Present only for rows that own a timeline dot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
}

impl ChatEntry {
    pub fn user(text: impl Into<String>, images: Vec<ChatImage>, time: u64) -> Self {
        Self {
            kind: "user".into(),
            text: Some(text.into()),
            images,
            event: None,
            time: Some(time),
        }
    }
    pub fn event(event: AgentEvent, time: Option<u64>) -> Self {
        Self {
            kind: "event".into(),
            text: None,
            images: Vec::new(),
            event: Some(event),
            time,
        }
    }
}

/// One durable row in the conversation file.
///
/// `fields` is flattened deliberately. A completed invocation is one readable
/// object such as `{ "role":"tool", "type":"run", "exitCode":0, ... }`.
/// Its arguments stay on the assistant's `tool_calls` row, so command, reason
/// and path are not copied into a second record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatLogMessage {
    pub role: String,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ChatImage>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub context: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty", rename = "tool_calls")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "tool_call_id")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<u64>,
    #[serde(default, flatten)]
    pub fields: Map<String, Value>,
}

impl ChatLogMessage {
    pub fn user(text: String, context: String, images: Vec<ChatImage>, time: u64) -> Self {
        let kind = match (text.is_empty(), images.is_empty()) {
            (false, true) => "text",
            (true, false) => "image",
            (false, false) => "image_text",
            (true, true) => "text",
        };
        Self {
            role: "user".into(),
            kind: kind.into(),
            text,
            images,
            context,
            tool_calls: Vec::new(),
            tool_call_id: None,
            time: Some(time),
            fields: Map::new(),
        }
    }

    fn assistant(message: &ChatMessage, time: u64) -> Self {
        Self {
            role: "assistant".into(),
            kind: "reply".into(),
            text: message.content.clone(),
            images: Vec::new(),
            context: String::new(),
            tool_calls: message.tool_calls.clone(),
            tool_call_id: None,
            time: Some(time),
            fields: Map::new(),
        }
    }

    pub fn event(event: AgentEvent, time: Option<u64>) -> Self {
        let role = if matches!(
            event,
            AgentEvent::Reasoning { .. } | AgentEvent::Reply { .. }
        ) {
            "assistant"
        } else {
            "tool"
        };
        let mut object = serde_json::to_value(&event)
            .ok()
            .and_then(|value| value.as_object().cloned())
            .unwrap_or_default();
        let kind = object
            .remove("type")
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_else(|| "event".into());
        let text = object
            .remove("text")
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        Self {
            role: role.into(),
            kind,
            text,
            images: Vec::new(),
            context: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            time,
            fields: object,
        }
    }

    fn tool(call: Option<&ToolCall>, observation: &ChatMessage, events: Vec<AgentEvent>) -> Self {
        let kind = call
            .map(|call| call.name.trim().to_lowercase())
            .filter(|name| ActionKind::parse(name).is_some())
            .unwrap_or_else(|| tool_name_from_events(&events).to_string());
        let mut fields = Map::new();
        let mut text = String::new();

        for event in events {
            match event {
                AgentEvent::Command {
                    command,
                    why,
                    verdict: _,
                    blocker: _,
                    unconfirmed,
                } => {
                    if let Some(unconfirmed) = unconfirmed {
                        fields.insert("unconfirmed".into(), json_value(unconfirmed));
                    }
                    if call.is_none() {
                        fields.insert("command".into(), Value::String(command));
                        fields.insert("why".into(), Value::String(why));
                    }
                }
                AgentEvent::Result {
                    exit_code,
                    output,
                    timed_out,
                    truncated,
                } => {
                    fields.insert("exitCode".into(), Value::from(exit_code));
                    fields.insert("output".into(), Value::String(output));
                    fields.insert("timedOut".into(), Value::Bool(timed_out));
                    fields.insert("truncated".into(), Value::Bool(truncated));
                }
                AgentEvent::File {
                    path,
                    exists,
                    size,
                    preview,
                    before,
                    after,
                    encoding,
                    line,
                    ..
                } => {
                    // Native calls already hold the requested path and content
                    // on the assistant row. Keep only facts discovered while
                    // planning the real file. JSON fallback has no tool_calls
                    // row to join against, so it still needs the full snapshot.
                    if call.is_none() {
                        fields.insert("path".into(), Value::String(path));
                        fields.insert("exists".into(), Value::Bool(exists));
                        fields.insert("size".into(), Value::String(size));
                        fields.insert("preview".into(), Value::String(preview));
                        fields.insert("before".into(), Value::String(before));
                        fields.insert("after".into(), Value::String(after));
                        fields.insert("encoding".into(), json_value(encoding));
                    }
                    if let Some(line) = line {
                        fields.insert("line".into(), Value::from(line));
                    }
                }
                AgentEvent::Memory {
                    scope,
                    text: memory,
                    outcome,
                    token,
                } => {
                    fields.insert("scope".into(), json_value(scope));
                    text = memory;
                    fields.insert("outcome".into(), json_value(outcome));
                    if let Some(token) = token {
                        fields.insert("token".into(), Value::String(token));
                    }
                }
                AgentEvent::Transfer {
                    sources, target, ..
                } => {
                    fields.insert("sources".into(), json_value(sources));
                    fields.insert("target".into(), Value::String(target));
                }
                AgentEvent::TransferProgress { progress } => {
                    fields.insert("progress".into(), json_value(progress));
                }
                AgentEvent::Skill {
                    id,
                    file,
                    outcome,
                    truncated,
                } => {
                    fields.insert("id".into(), Value::String(id));
                    fields.insert("file".into(), Value::String(file));
                    fields.insert("outcome".into(), json_value(outcome));
                    if let Some(truncated) = truncated {
                        fields.insert("truncated".into(), Value::Bool(truncated));
                    }
                }
                AgentEvent::Trusted { dir } => {
                    fields.insert("trustedDir".into(), Value::String(dir));
                }
                AgentEvent::Refused { command, reasons } => {
                    fields.insert("refused".into(), Value::Bool(true));
                    fields.insert("reasons".into(), json_value(reasons));
                    if call.is_none() {
                        fields.insert("command".into(), Value::String(command));
                    }
                }
                AgentEvent::Declined { command } => {
                    fields.insert("declined".into(), Value::Bool(true));
                    if call.is_none() {
                        fields.insert("command".into(), Value::String(command));
                    }
                }
                _ => {}
            }
        }

        let mut record = Self {
            role: "tool".into(),
            kind,
            text,
            images: Vec::new(),
            context: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: observation.tool_call_id.clone(),
            time: None,
            fields,
        };
        if record.result_observation().as_deref() != Some(observation.content.as_str()) {
            record.fields.insert(
                "observation".into(),
                Value::String(observation.content.clone()),
            );
        }
        record
    }

    fn model_input(&self) -> ChatMessage {
        ChatMessage::user_with_images(
            crate::ai::prompt::context_turn(&self.context, &self.text),
            self.images.clone(),
        )
    }

    fn as_event(&self) -> Option<AgentEvent> {
        if self.role == "user" || (self.role == "assistant" && self.kind == "reply") {
            return None;
        }
        let mut object = self.fields.clone();
        object.insert("type".into(), Value::String(self.kind.clone()));
        if !self.text.is_empty() {
            object.insert("text".into(), Value::String(self.text.clone()));
        }
        serde_json::from_value(Value::Object(object)).ok()
    }

    fn is_tool_call(&self) -> bool {
        self.role == "tool" && ActionKind::parse(&self.kind).is_some()
    }

    fn result_observation(&self) -> Option<String> {
        let exit_code = field(&self.fields, "exitCode")?;
        let output = field(&self.fields, "output")?;
        let timed_out = field(&self.fields, "timedOut")?;
        let truncated = field(&self.fields, "truncated")?;
        Some(describe_result(&CommandResult {
            output,
            exit_code,
            timed_out,
            truncated,
        }))
    }

    fn observation(&self) -> Option<String> {
        if self.role == "tool" && self.kind == "observation" {
            return Some(self.text.clone());
        }
        self.fields
            .get("observation")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.result_observation())
    }

    fn replay_events(&self, call: Option<&ToolCall>) -> Vec<AgentEvent> {
        let action = call
            .and_then(action_from_call)
            .unwrap_or_else(|| action_from_record(self));
        let Some(kind) = action.action else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let why = action.why.clone().unwrap_or_default();
        let command = invocation_label(kind, &action);
        if !matches!(kind, ActionKind::Remember | ActionKind::Skill) && !command.is_empty() {
            events.push(AgentEvent::Command {
                command: command.clone(),
                why,
                verdict: Verdict::Auto,
                blocker: None,
                unconfirmed: field(&self.fields, "unconfirmed"),
            });
        }

        if let Some(file_kind) = kind.as_file_op() {
            if self.fields.contains_key("line") || self.fields.contains_key("path") {
                let path = action
                    .path
                    .clone()
                    .or_else(|| field(&self.fields, "path"))
                    .unwrap_or_default();
                let content = action.content.clone().unwrap_or_default();
                events.push(AgentEvent::File {
                    kind: file_kind,
                    path,
                    exists: field(&self.fields, "exists")
                        .unwrap_or(!matches!(file_kind, FileOpKind::Write)),
                    size: field(&self.fields, "size").unwrap_or_default(),
                    preview: field(&self.fields, "preview").unwrap_or_else(|| content.clone()),
                    before: field(&self.fields, "before")
                        .or_else(|| action.old_text.clone())
                        .unwrap_or_default(),
                    after: field(&self.fields, "after")
                        .or_else(|| action.new_text.clone())
                        .unwrap_or_else(|| content.clone()),
                    encoding: field(&self.fields, "encoding").unwrap_or(FileEncoding::Utf8),
                    line: field(&self.fields, "line"),
                });
            }
        } else if let Some(transfer_kind) = kind.as_transfer() {
            if let (Some(sources), Some(target)) = (
                field(&self.fields, "sources"),
                field(&self.fields, "target"),
            ) {
                events.push(AgentEvent::Transfer {
                    kind: transfer_kind,
                    sources,
                    target,
                });
            }
        } else if kind == ActionKind::Remember {
            if let (Some(scope), Some(text), Some(outcome)) = (
                field(&self.fields, "scope"),
                (!self.text.is_empty()).then(|| self.text.clone()),
                field(&self.fields, "outcome"),
            ) {
                events.push(AgentEvent::Memory {
                    scope,
                    text,
                    outcome,
                    token: field(&self.fields, "token"),
                });
            }
        } else if kind == ActionKind::Skill {
            if let (Some(id), Some(file), Some(outcome)) = (
                field(&self.fields, "id"),
                field(&self.fields, "file"),
                field(&self.fields, "outcome"),
            ) {
                events.push(AgentEvent::Skill {
                    id,
                    file,
                    outcome,
                    truncated: field(&self.fields, "truncated"),
                });
            }
        }

        if let Some(progress) = field(&self.fields, "progress") {
            events.push(AgentEvent::TransferProgress { progress });
        }

        if let Some(dir) = field(&self.fields, "trustedDir") {
            events.push(AgentEvent::Trusted { dir });
        }
        if field(&self.fields, "refused").unwrap_or(false) {
            events.push(AgentEvent::Refused {
                command,
                reasons: field(&self.fields, "reasons").unwrap_or_default(),
            });
        } else if field(&self.fields, "declined").unwrap_or(false) {
            events.push(AgentEvent::Declined { command });
        } else if let (Some(exit_code), Some(output), Some(timed_out), Some(truncated)) = (
            field(&self.fields, "exitCode"),
            field(&self.fields, "output"),
            field(&self.fields, "timedOut"),
            field(&self.fields, "truncated"),
        ) {
            events.push(AgentEvent::Result {
                exit_code,
                output,
                timed_out,
                truncated,
            });
        }
        events
    }

    fn model_observation(message: &ChatMessage) -> Self {
        Self {
            role: "tool".into(),
            kind: "observation".into(),
            text: message.content.clone(),
            images: Vec::new(),
            context: String::new(),
            tool_calls: Vec::new(),
            tool_call_id: message.tool_call_id.clone(),
            time: None,
            fields: Map::new(),
        }
    }
}

/// What this conversation has cost so far, across every step of every task in it.
///
/// Kept with the conversation rather than with the panel so that reopening one
/// shows what it actually cost rather than starting again from zero, and so that
/// the number means the same thing after a reload as it did before.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatUsage {
    pub prompt: u64,
    pub completion: u64,
    pub requests: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRecord {
    pub version: u32,
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    /// The opening message, shortened. Written once and then left alone.
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    pub messages: Vec<ChatLogMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
    /// Live UI fragments waiting for the model observation that completes one
    /// invocation. They are deliberately never serialized: the durable form is
    /// the single unified tool row built by `sync_history`.
    #[serde(skip)]
    pub pending_tool_events: Vec<AgentEvent>,
}

impl ChatRecord {
    pub fn entries(&self) -> Vec<ChatEntry> {
        let mut entries = Vec::new();
        let mut pending_response_time = None;
        let mut calls = VecDeque::new();
        for message in &self.messages {
            if message.role == "user" {
                pending_response_time = None;
                entries.push(ChatEntry::user(
                    message.text.clone(),
                    message.images.clone(),
                    message.time.unwrap_or_default(),
                ));
                continue;
            }
            if message.role == "assistant" && message.kind == "reply" {
                calls.extend(message.tool_calls.iter());
                if message.text.is_empty() {
                    // A tool-only response still owns a timeline point. Its first
                    // visible tool row borrows the time for display; the durable
                    // tool record itself remains untimed.
                    pending_response_time = message.time;
                } else {
                    entries.push(ChatEntry::event(
                        AgentEvent::Reply {
                            text: message.text.clone(),
                        },
                        message.time,
                    ));
                }
                continue;
            }
            if message.is_tool_call() {
                let call = message
                    .tool_call_id
                    .as_deref()
                    .and_then(|id| calls.iter().find(|call| call.id == id).copied())
                    .or_else(|| calls.pop_front());
                if let Some(call) = call {
                    if let Some(index) = calls.iter().position(|queued| queued.id == call.id) {
                        calls.remove(index);
                    }
                }
                for event in message.replay_events(call) {
                    let time = message.time.or_else(|| pending_response_time.take());
                    entries.push(ChatEntry::event(event, time));
                }
                continue;
            }
            if let Some(event) = message.as_event() {
                let time = message.time.or_else(|| pending_response_time.take());
                entries.push(ChatEntry::event(event, time));
            }
        }
        entries
    }

    pub fn history(&self) -> Vec<ChatMessage> {
        let mut messages = Vec::new();
        let mut pending_calls = VecDeque::new();
        for record in &self.messages {
            if record.role == "user" {
                messages.push(record.model_input());
                continue;
            }
            if record.role == "assistant" && record.kind == "reply" {
                let model_text = record
                    .fields
                    .get("modelText")
                    .and_then(Value::as_str)
                    .unwrap_or(&record.text);
                pending_calls = record
                    .tool_calls
                    .iter()
                    .map(|call| call.id.clone())
                    .collect();
                messages.push(if record.tool_calls.is_empty() {
                    ChatMessage::assistant(model_text)
                } else {
                    ChatMessage::assistant_calls(model_text, record.tool_calls.clone())
                });
                continue;
            }
            if !record.is_tool_call() && !(record.role == "tool" && record.kind == "observation") {
                continue;
            }
            let Some(observation) = record.observation() else {
                continue;
            };
            let stored_id = record.tool_call_id.clone();
            if let Some(id) = stored_id.or_else(|| pending_calls.pop_front()) {
                messages.push(ChatMessage::tool_result(id, observation));
            } else {
                // JSON-tool fallback observations are user-role messages in the
                // model protocol, but remain `role:tool` in the readable file.
                messages.push(ChatMessage::user(observation));
            }
        }
        messages
    }

    /// Merge the model-facing facts into the same records the page reads.
    /// Assistant replies are first-class records. Tool observations annotate the
    /// terminal tool event that already owns their structured output instead of
    /// being stored again in a parallel `modelMessages` array.
    pub fn sync_history(&mut self, history: &[ChatMessage]) {
        let assistants: Vec<&ChatMessage> = history
            .iter()
            .filter(|message| message.role == ChatRole::Assistant)
            .collect();
        let existing: Vec<usize> = self
            .messages
            .iter()
            .enumerate()
            .filter_map(|(index, message)| {
                (message.role == "assistant" && message.kind == "reply").then_some(index)
            })
            .collect();
        for (index, assistant) in assistants.iter().enumerate() {
            if let Some(stored) = existing.get(index).copied() {
                let record = &mut self.messages[stored];
                if record.text != assistant.content {
                    record
                        .fields
                        .insert("modelText".into(), Value::String(assistant.content.clone()));
                }
                record.tool_calls = assistant.tool_calls.clone();
            } else {
                self.messages
                    .push(ChatLogMessage::assistant(assistant, now_ms()));
            }
        }

        let mut pending_groups = self.take_completed_tool_groups();

        let mut cursor = 0;
        let mut next_input = 0;
        for message in history {
            if message.role == ChatRole::User {
                let input = self
                    .messages
                    .iter()
                    .filter(|record| record.role == "user")
                    .nth(next_input)
                    .map(ChatLogMessage::model_input);
                if input.as_ref() == Some(message) {
                    if let Some(relative) = self.messages[cursor..]
                        .iter()
                        .position(|record| record.role == "user")
                    {
                        cursor += relative + 1;
                    }
                    next_input += 1;
                    continue;
                }
            }
            if message.role == ChatRole::Assistant {
                if let Some(relative) = self.messages[cursor..]
                    .iter()
                    .position(|record| record.role == "assistant" && record.kind == "reply")
                {
                    cursor += relative + 1;
                }
                continue;
            }
            if !matches!(message.role, ChatRole::User | ChatRole::Tool) {
                continue;
            }

            let boundary = self.messages[cursor..]
                .iter()
                .position(|record| {
                    record.role == "user" || (record.role == "assistant" && record.kind == "reply")
                })
                .map(|relative| cursor + relative)
                .unwrap_or(self.messages.len());
            let target = self.messages[cursor..boundary]
                .iter()
                .position(|record| {
                    record.is_tool_call() || (record.role == "tool" && record.kind == "observation")
                })
                .map(|relative| cursor + relative);
            let stored = if let Some(target) = target {
                target
            } else if let Some(events) = pending_groups.pop_front() {
                let call = message
                    .tool_call_id
                    .as_deref()
                    .and_then(|id| self.find_tool_call(id));
                self.messages.insert(
                    boundary,
                    ChatLogMessage::tool(call.as_ref(), message, events),
                );
                boundary
            } else {
                self.messages
                    .insert(boundary, ChatLogMessage::model_observation(message));
                boundary
            };
            let record = &mut self.messages[stored];
            if let Some(id) = &message.tool_call_id {
                record.tool_call_id = Some(id.clone());
            }
            let reconstructed = record.result_observation();
            if record.kind != "observation"
                && reconstructed.as_deref() != Some(message.content.as_str())
            {
                record
                    .fields
                    .insert("observation".into(), Value::String(message.content.clone()));
            } else if record.kind != "observation" {
                record.fields.remove("observation");
            }
            cursor = stored + 1;
        }
        for group in pending_groups {
            self.pending_tool_events.extend(group);
        }
    }

    pub fn push_tool_event(&mut self, event: AgentEvent) {
        self.pending_tool_events.push(event);
    }

    pub fn discard_incomplete_tool(&mut self) {
        self.pending_tool_events.clear();
    }

    fn find_tool_call(&self, id: &str) -> Option<ToolCall> {
        self.messages
            .iter()
            .flat_map(|message| message.tool_calls.iter())
            .find(|call| call.id == id)
            .cloned()
    }

    fn take_completed_tool_groups(&mut self) -> VecDeque<Vec<AgentEvent>> {
        let mut groups = VecDeque::new();
        let mut current = Vec::new();
        for event in std::mem::take(&mut self.pending_tool_events) {
            let terminal = is_terminal_tool_event(&event);
            current.push(event);
            if terminal {
                groups.push_back(std::mem::take(&mut current));
            }
        }
        self.pending_tool_events = current;
        groups
    }
}

fn json_value<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

fn field<T: for<'de> Deserialize<'de>>(fields: &Map<String, Value>, key: &str) -> Option<T> {
    serde_json::from_value(fields.get(key)?.clone()).ok()
}

fn is_terminal_tool_event(event: &AgentEvent) -> bool {
    matches!(
        event,
        AgentEvent::Result { .. }
            | AgentEvent::Memory { .. }
            | AgentEvent::Skill { .. }
            | AgentEvent::Refused { .. }
            | AgentEvent::Declined { .. }
    )
}

fn tool_name_from_events(events: &[AgentEvent]) -> &'static str {
    for event in events {
        match event {
            AgentEvent::File { kind, .. } => return kind.tag(),
            AgentEvent::Transfer { kind, .. } => {
                return match kind {
                    TransferKind::Upload => "upload",
                    TransferKind::Download => "download",
                };
            }
            AgentEvent::Memory { .. } => return "remember",
            AgentEvent::Skill { .. } => return "skill",
            AgentEvent::Command { command, .. } | AgentEvent::Refused { command, .. } => {
                if let Some(kind) = command
                    .split_whitespace()
                    .next()
                    .and_then(ActionKind::parse)
                    .filter(|kind| *kind != ActionKind::Run)
                {
                    return kind.tag();
                }
            }
            _ => {}
        }
    }
    "run"
}

fn action_from_record(record: &ChatLogMessage) -> AgentAction {
    AgentAction {
        action: ActionKind::parse(&record.kind),
        command: field(&record.fields, "command"),
        why: field(&record.fields, "why"),
        text: (!record.text.is_empty()).then(|| record.text.clone()),
        path: field(&record.fields, "path"),
        paths: field(&record.fields, "sources").unwrap_or_default(),
        to: field(&record.fields, "target"),
        name: field(&record.fields, "id"),
        file: field(&record.fields, "file"),
        scope: field(&record.fields, "scope"),
        ..AgentAction::default()
    }
}

fn invocation_label(kind: ActionKind, action: &AgentAction) -> String {
    match kind {
        ActionKind::Run => action.command.clone().unwrap_or_default(),
        ActionKind::Write | ActionKind::Append | ActionKind::Edit => {
            if action.path.as_deref().unwrap_or_default().is_empty() {
                return action.command.clone().unwrap_or_default();
            }
            format!(
                "{} {}",
                kind.tag(),
                action.path.as_deref().unwrap_or_default()
            )
        }
        ActionKind::Upload | ActionKind::Download => {
            if action.paths.is_empty() {
                return action.command.clone().unwrap_or_default();
            }
            let mut parts = vec![kind.tag().to_string(), action.paths.join(" ")];
            if let Some(target) = action.to.as_deref().filter(|target| !target.is_empty()) {
                parts.push(format!("-> {target}"));
            }
            parts
                .into_iter()
                .filter(|part| !part.trim().is_empty())
                .collect::<Vec<_>>()
                .join(" ")
        }
        ActionKind::Remember | ActionKind::Skill => String::new(),
    }
}

/// A row in the history list: everything but the two heavy arrays.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// The opening message, cut to something that fits a row in the list.
pub fn title_for(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > MAX_TITLE {
        format!("{}…", line.chars().take(MAX_TITLE).collect::<String>())
    } else {
        line
    }
}

/// What the tab header says.
///
/// The conversation names the tab and the machine tells two of them apart, so
/// when the two do not fit it is the conversation that gets cut and never the
/// machine. Before the first message there is no conversation to name it after,
/// which is what the fallback is for.
pub fn panel_title(chat_title: &str, server_name: &str, fallback: &str) -> String {
    let named = if chat_title.trim().is_empty() {
        fallback
    } else {
        chat_title.trim()
    };
    let short = if named.chars().count() > MAX_TAB_TITLE {
        format!("{}…", named.chars().take(MAX_TAB_TITLE).collect::<String>())
    } else {
        named.to_string()
    };
    format!("{short} ({server_name})")
}

pub struct ChatStore {
    dir: PathBuf,
}

impl ChatStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    fn file_for(&self, id: &str) -> PathBuf {
        // Ids are made here rather than typed, but this is a filename either way
        // and a stray separator would put a conversation somewhere else entirely.
        let safe: String = id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        // Leading dots go for the same reason they do in memory's filenames: a
        // conversation must not be able to name a hidden file.
        let safe = safe.trim_start_matches('.');
        self.dir.join(format!("{safe}.json"))
    }

    /// Writes the conversation as it now stands.
    ///
    /// The TypeScript coalesced these behind a 400ms timer because every event
    /// rewrote the whole file from the extension host's single thread. Here the
    /// write happens on a blocking-safe path off the session's own task and the
    /// file is small, so the timer would only be a way to lose the last exchange
    /// of a conversation to a crash.
    pub fn save(&self, record: &mut ChatRecord) {
        record.updated_at = now_ms();
        let Ok(body) = serde_json::to_string_pretty(record) else {
            return;
        };
        // A conversation that cannot be saved is not a reason to interrupt one
        // that is being had. The panel keeps working; the history is what suffers.
        let _ = atomic::write(&self.file_for(&record.id), &body);
    }

    pub fn list(&self) -> Vec<ChatSummary> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };

        let mut summaries: Vec<ChatSummary> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
            })
            .filter_map(|entry| self.read(&entry.path()))
            .map(|record| ChatSummary {
                id: record.id,
                server_id: record.server_id,
                server_name: record.server_name,
                title: record.title,
                created_at: record.created_at,
                updated_at: record.updated_at,
                usage: record.usage,
            })
            .collect();
        summaries.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        summaries.truncate(MAX_LISTED);
        summaries
    }

    pub fn load(&self, id: &str) -> Option<ChatRecord> {
        self.read(&self.file_for(id))
    }

    pub fn remove(&self, id: &str) {
        // Already gone is what was wanted.
        let _ = std::fs::remove_file(self.file_for(id));
    }

    fn read(&self, file: &std::path::Path) -> Option<ChatRecord> {
        let text = std::fs::read_to_string(file).ok()?;
        let record: ChatRecord = serde_json::from_str(&text).ok()?;
        if record.version != 1 || record.id.is_empty() {
            return None;
        }
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::types::{TransferCurrent, TransferOverall, TransferPhase, TransferProgress};

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("tshell-chat-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> ChatStore {
            ChatStore::new(&self.0)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn record(id: &str, title: &str) -> ChatRecord {
        ChatRecord {
            version: 1,
            id: id.into(),
            server_id: "s".into(),
            server_name: "web-1".into(),
            title: title.into(),
            created_at: 1,
            updated_at: 1,
            messages: vec![ChatLogMessage::user(
                "hello".into(),
                String::new(),
                Vec::new(),
                1,
            )],
            usage: None,
            pending_tool_events: Vec::new(),
        }
    }

    #[test]
    fn a_saved_conversation_comes_back_whole() {
        let temp = Temp::new("roundtrip");
        let store = temp.store();
        let mut saved = record("abc", "hello");
        let history = vec![
            saved.messages[0].model_input(),
            ChatMessage::assistant("done"),
        ];
        saved.sync_history(&history);
        saved
            .messages
            .push(ChatLogMessage::event(AgentEvent::Stopped, None));
        store.save(&mut saved);

        let loaded = store.load("abc").unwrap();
        assert_eq!(loaded.title, "hello");
        assert_eq!(loaded.messages.len(), 3);
        assert_eq!(loaded.entries().len(), 3);
        assert_eq!(loaded.history().len(), 2);
        assert_eq!(loaded.entries()[2].event, Some(AgentEvent::Stopped));
    }

    #[test]
    fn one_message_holds_text_and_images_without_a_second_user_record() {
        let mut saved = record("abc", "hello");
        saved.messages[0] = ChatLogMessage::user(
            "look".into(),
            "cwd: /tmp".into(),
            vec![ChatImage {
                media_type: "image/png".into(),
                data: "iVBORw==".into(),
            }],
            1234,
        );
        let json = serde_json::to_value(&saved).unwrap();
        assert!(json.get("turns").is_none());
        assert_eq!(json["version"], 1);
        assert_eq!(json["messages"][0]["role"], "user");
        assert_eq!(json["messages"][0]["type"], "image_text");
        assert_eq!(json["messages"][0]["images"][0]["mediaType"], "image/png");
        assert_eq!(json["messages"][0]["time"], 1234);
        assert_eq!(saved.history().len(), 1, "the input is reconstructed once");
        assert_eq!(saved.entries().len(), 1, "the same input draws the page");
    }

    #[test]
    fn one_messages_array_restores_native_calls_without_model_messages() {
        let mut saved = record("abc", "hello");
        let call = ToolCall {
            id: "call_1".into(),
            name: "run".into(),
            arguments: r#"{"command":"pwd","why":"cwd"}"#.into(),
        };
        let assistant = ChatMessage::assistant_calls("checking", vec![call]);
        saved.sync_history(&[saved.messages[0].model_input(), assistant.clone()]);
        saved.push_tool_event(AgentEvent::Command {
            command: "pwd".into(),
            why: "cwd".into(),
            verdict: crate::ai::policy::command::Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });
        saved.push_tool_event(AgentEvent::Result {
            exit_code: 0,
            output: "/home/trade".into(),
            timed_out: false,
            truncated: false,
        });
        let history = vec![
            saved.messages[0].model_input(),
            assistant,
            ChatMessage::tool_result("call_1", "EXIT: 0\nOUTPUT:\n/home/trade"),
        ];
        saved.sync_history(&history);

        assert_eq!(saved.history(), history);
        let json = serde_json::to_value(&saved).unwrap();
        assert!(json["messages"][1].get("modelMessages").is_none());
        assert_eq!(json["messages"][1]["tool_calls"][0]["id"], "call_1");
        assert_eq!(json["messages"].as_array().unwrap().len(), 3);
        assert_eq!(json["messages"][2]["role"], "tool");
        assert_eq!(json["messages"][2]["type"], "run");
        assert_eq!(json["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(json["messages"][2]["exitCode"], 0);
        assert_eq!(json["messages"][2]["output"], "/home/trade");
        assert_eq!(json["messages"][2]["timedOut"], false);
        assert_eq!(json["messages"][2]["truncated"], false);
        assert_eq!(json["messages"][2].as_object().unwrap().len(), 7);
        assert!(json["messages"][2].get("command").is_none());
        assert!(json["messages"][2].get("why").is_none());
        assert!(json["messages"][2].get("observation").is_none());
        assert_eq!(saved.entries().len(), 4);
    }

    #[test]
    fn a_batch_keeps_exactly_one_row_per_tool_and_uses_each_tool_name() {
        let mut saved = record("batch", "tools");
        let run = ToolCall {
            id: "call_run".into(),
            name: "run".into(),
            arguments: r#"{"command":"pwd","why":"cwd"}"#.into(),
        };
        let edit = ToolCall {
            id: "call_edit".into(),
            name: "edit".into(),
            arguments: r#"{"path":"/tmp/a.rs","old":"a","new":"b","why":"fix"}"#.into(),
        };
        let assistant = ChatMessage::assistant_calls("", vec![run, edit]);
        saved.sync_history(&[saved.messages[0].model_input(), assistant.clone()]);

        saved.push_tool_event(AgentEvent::Command {
            command: "pwd".into(),
            why: "cwd".into(),
            verdict: Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });
        saved.push_tool_event(AgentEvent::Result {
            exit_code: 0,
            output: "/tmp".into(),
            timed_out: false,
            truncated: false,
        });
        saved.push_tool_event(AgentEvent::Command {
            command: "edit /tmp/a.rs".into(),
            why: "fix".into(),
            verdict: Verdict::Confirm,
            blocker: None,
            unconfirmed: Some(crate::ai::types::Unconfirmed::Auto),
        });
        saved.push_tool_event(AgentEvent::File {
            kind: FileOpKind::Edit,
            path: "/tmp/a.rs".into(),
            exists: true,
            size: "1 B".into(),
            preview: "b".into(),
            before: "a".into(),
            after: "b".into(),
            encoding: FileEncoding::Utf8,
            line: Some(4),
        });
        saved.push_tool_event(AgentEvent::Result {
            exit_code: 0,
            output: "updated".into(),
            timed_out: false,
            truncated: false,
        });
        let history = vec![
            saved.messages[0].model_input(),
            assistant,
            ChatMessage::tool_result("call_run", "EXIT: 0\nOUTPUT:\n/tmp"),
            ChatMessage::tool_result("call_edit", "EXIT: 0\nOUTPUT:\nupdated"),
        ];
        saved.sync_history(&history);

        let json = serde_json::to_value(&saved).unwrap();
        let tools: Vec<&Value> = json["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .collect();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0]["type"], "run");
        assert_eq!(tools[1]["type"], "edit");
        assert!(tools.iter().all(|message| !matches!(
            message["type"].as_str(),
            Some("command" | "file" | "result")
        )));
        for redundant in [
            "path", "exists", "size", "preview", "before", "after", "encoding",
        ] {
            assert!(tools[1].get(redundant).is_none(), "duplicate {redundant}");
        }

        let loaded: ChatRecord = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.history(), history);
        assert_eq!(loaded.entries().len(), 6);
        let edit = loaded
            .entries()
            .into_iter()
            .find_map(|entry| match entry.event {
                Some(AgentEvent::File {
                    kind,
                    path,
                    before,
                    after,
                    line,
                    ..
                }) => Some((kind, path, before, after, line)),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            edit,
            (
                FileOpKind::Edit,
                "/tmp/a.rs".into(),
                "a".into(),
                "b".into(),
                Some(4)
            )
        );
    }

    #[test]
    fn native_write_edit_and_append_rebuild_their_details_from_tool_calls() {
        let cases = [
            (
                "write",
                FileOpKind::Write,
                r#"{"path":"/tmp/write.rs","content":"new\n","why":"write it"}"#,
                "",
                "new\n",
                "new\n",
            ),
            (
                "append",
                FileOpKind::Append,
                r#"{"path":"/tmp/append.rs","content":"tail\n","why":"append it"}"#,
                "",
                "tail\n",
                "tail\n",
            ),
            (
                "edit",
                FileOpKind::Edit,
                r#"{"path":"/tmp/edit.rs","old":"before","new":"after","why":"edit it"}"#,
                "before",
                "after",
                "",
            ),
        ];

        for (name, kind, arguments, expected_before, expected_after, expected_preview) in cases {
            let call = ToolCall {
                id: format!("call_{name}"),
                name: name.into(),
                arguments: arguments.into(),
            };
            let path = format!("/tmp/{name}.rs");
            let observation = ChatMessage::tool_result(&call.id, "EXIT: 0\nOUTPUT:\nok");
            let record = ChatLogMessage::tool(
                Some(&call),
                &observation,
                vec![
                    AgentEvent::File {
                        kind,
                        path: path.clone(),
                        exists: kind != FileOpKind::Write,
                        size: "100 B".into(),
                        preview: expected_preview.into(),
                        before: expected_before.into(),
                        after: expected_after.into(),
                        encoding: FileEncoding::Gb18030,
                        line: Some(7),
                    },
                    AgentEvent::Result {
                        exit_code: 0,
                        output: "ok".into(),
                        timed_out: false,
                        truncated: false,
                    },
                ],
            );
            let json = serde_json::to_value(&record).unwrap();
            for redundant in [
                "path", "exists", "size", "preview", "before", "after", "encoding",
            ] {
                assert!(
                    json.get(redundant).is_none(),
                    "{name} duplicated {redundant}"
                );
            }
            assert!(json.get("planned").is_none());
            assert_eq!(json["line"], 7);

            let loaded: ChatLogMessage = serde_json::from_value(json).unwrap();
            let file = loaded
                .replay_events(Some(&call))
                .into_iter()
                .find_map(|event| match event {
                    AgentEvent::File {
                        kind,
                        path,
                        preview,
                        before,
                        after,
                        line,
                        ..
                    } => Some((kind, path, preview, before, after, line)),
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                file,
                (
                    kind,
                    path,
                    expected_preview.into(),
                    expected_before.into(),
                    expected_after.into(),
                    Some(7)
                )
            );
        }
    }

    #[test]
    fn a_transfer_row_keeps_the_final_file_and_byte_progress_for_replay() {
        let mut saved = record("transfer", "download");
        let call = ToolCall {
            id: "call_download".into(),
            name: "download".into(),
            arguments: r#"{"paths":["/tmp/a","/tmp/b"],"to":"C:\\Temp","why":"fetch"}"#.into(),
        };
        let assistant = ChatMessage::assistant_calls("", vec![call]);
        saved.sync_history(&[saved.messages[0].model_input(), assistant.clone()]);
        saved.push_tool_event(AgentEvent::Command {
            command: "download /tmp/a /tmp/b -> C:\\Temp".into(),
            why: "fetch".into(),
            verdict: Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });
        saved.push_tool_event(AgentEvent::Transfer {
            kind: TransferKind::Download,
            sources: vec!["/tmp/a".into(), "/tmp/b".into()],
            target: "C:\\Temp".into(),
        });
        let progress = TransferProgress {
            phase: TransferPhase::Transferring,
            overall: TransferOverall {
                done_files: 2,
                total_files: 2,
                done_bytes: 19_456,
                total_bytes: 19_456,
            },
            current: TransferCurrent {
                name: "b".into(),
                transferred: 2_048,
                total: 2_048,
            },
        };
        saved.push_tool_event(AgentEvent::TransferProgress {
            progress: progress.clone(),
        });
        saved.push_tool_event(AgentEvent::Result {
            exit_code: 0,
            output: "Downloaded 2 files".into(),
            timed_out: false,
            truncated: false,
        });
        let history = vec![
            saved.messages[0].model_input(),
            assistant,
            ChatMessage::tool_result("call_download", "EXIT: 0\nOUTPUT:\nDownloaded 2 files"),
        ];
        saved.sync_history(&history);

        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["messages"][2]["type"], "download");
        assert_eq!(
            json["messages"][2]["progress"]["overall"]["totalBytes"],
            19_456
        );
        assert_eq!(json["messages"][2]["progress"]["current"]["total"], 2_048);

        let loaded: ChatRecord = serde_json::from_value(json).unwrap();
        assert_eq!(loaded.history(), history);
        assert!(loaded.entries().iter().any(|entry| {
            matches!(
                entry.event.as_ref(),
                Some(AgentEvent::TransferProgress { progress: restored }) if restored == &progress
            )
        }));
    }

    #[test]
    fn a_reply_after_a_tool_batch_keeps_its_own_timeline_time() {
        let mut saved = record("times", "response boundaries");
        let input = saved.messages[0].model_input();
        let call = ToolCall {
            id: "call_1".into(),
            name: "run".into(),
            arguments: r#"{"command":"pwd","why":"cwd"}"#.into(),
        };
        let tools = ChatMessage::assistant_calls("", vec![call]);
        saved.sync_history(&[input.clone(), tools.clone()]);
        saved.push_tool_event(AgentEvent::Command {
            command: "pwd".into(),
            why: "cwd".into(),
            verdict: Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });
        saved.push_tool_event(AgentEvent::Result {
            exit_code: 0,
            output: "/tmp".into(),
            timed_out: false,
            truncated: false,
        });
        let result = ChatMessage::tool_result("call_1", "EXIT: 0\nOUTPUT:\n/tmp");
        saved.sync_history(&[input.clone(), tools.clone(), result.clone()]);
        let final_reply = ChatMessage::assistant("report complete");
        saved.sync_history(&[input, tools, result, final_reply]);

        let mut assistant_times = [10_u64, 20_u64].into_iter();
        for message in &mut saved.messages {
            if message.role == "assistant" && message.kind == "reply" {
                message.time = assistant_times.next();
            }
        }
        let entries = saved.entries();
        let command = entries
            .iter()
            .find(|entry| matches!(entry.event.as_ref(), Some(AgentEvent::Command { .. })))
            .unwrap();
        let reply = entries
            .iter()
            .find(|entry| matches!(entry.event.as_ref(), Some(AgentEvent::Reply { .. })))
            .unwrap();
        assert_eq!(command.time, Some(10));
        assert_eq!(reply.time, Some(20));
    }

    #[test]
    fn an_internal_observation_stays_in_sequence_without_becoming_a_ui_entry() {
        let mut saved = record("abc", "hello");
        let input = saved.messages[0].model_input();
        let first = ChatMessage::assistant("I need a corrected action.");
        let internal = ChatMessage::user("The run action needs a command.");
        let call = ToolCall {
            id: "call_2".into(),
            name: "run".into(),
            arguments: r#"{"command":"pwd","why":"cwd"}"#.into(),
        };
        let second = ChatMessage::assistant_calls("trying again", vec![call]);

        saved.sync_history(&[input.clone(), first.clone(), internal.clone()]);
        saved.sync_history(&[
            input.clone(),
            first.clone(),
            internal.clone(),
            second.clone(),
        ]);
        saved.push_tool_event(AgentEvent::Command {
            command: "pwd".into(),
            why: "cwd".into(),
            verdict: crate::ai::policy::command::Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });
        saved.push_tool_event(AgentEvent::Result {
            exit_code: 0,
            output: "/tmp".into(),
            timed_out: false,
            truncated: false,
        });
        let history = vec![
            input,
            first,
            internal,
            second,
            ChatMessage::tool_result("call_2", "EXIT: 0\nOUTPUT:\n/tmp"),
        ];
        saved.sync_history(&history);

        assert_eq!(saved.history(), history);
        assert!(saved
            .messages
            .iter()
            .any(|message| message.kind == "observation"));
        assert_eq!(
            saved.entries().len(),
            5,
            "the internal observation is not drawn"
        );
    }

    #[test]
    fn only_timeline_records_have_time_and_usage_exists_only_at_the_root() {
        let mut saved = record("abc", "hello");
        saved.messages.push(ChatLogMessage::event(
            AgentEvent::Reasoning {
                text: "checking".into(),
            },
            Some(2),
        ));
        saved.sync_history(&[
            saved.messages[0].model_input(),
            ChatMessage::assistant("done"),
        ]);
        saved.push_tool_event(AgentEvent::Command {
            command: "pwd".into(),
            why: "cwd".into(),
            verdict: crate::ai::policy::command::Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        });
        saved.usage = Some(ChatUsage {
            prompt: 10,
            completion: 2,
            requests: 1,
        });

        let json = serde_json::to_value(&saved).unwrap();
        assert_eq!(json["messages"][0]["time"], 1);
        assert_eq!(json["messages"][1]["time"], 2);
        assert!(json["messages"][2]["time"].is_number());
        assert_eq!(json["messages"].as_array().unwrap().len(), 3);
        assert!(json.get("pendingToolEvents").is_none());
        assert_eq!(json["usage"]["requests"], 1);
        assert!(json.get("turns").is_none());
        assert!(json.get("formatVersion").is_none());
    }

    #[test]
    fn saving_stamps_the_time_it_was_written() {
        let temp = Temp::new("stamp");
        let store = temp.store();
        let mut saved = record("abc", "hello");
        store.save(&mut saved);
        assert!(saved.updated_at > 1);
    }

    #[test]
    fn the_list_is_newest_first_and_carries_no_heavy_arrays() {
        let temp = Temp::new("list");
        let store = temp.store();
        let mut older = record("older", "first");
        store.save(&mut older);
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut newer = record("newer", "second");
        store.save(&mut newer);

        let listed = store.list();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].id, "newer");
        assert_eq!(listed[1].id, "older");

        let json = serde_json::to_value(&listed[0]).unwrap();
        assert!(json.get("entries").is_none());
        assert!(json.get("messages").is_none());
        assert!(json.get("turns").is_none());
    }

    #[test]
    fn removing_a_conversation_takes_it_off_the_list() {
        let temp = Temp::new("remove");
        let store = temp.store();
        let mut saved = record("abc", "hello");
        store.save(&mut saved);
        store.remove("abc");
        assert!(store.load("abc").is_none());
        assert!(store.list().is_empty());
        // Removing it twice is not an error.
        store.remove("abc");
    }

    #[test]
    fn a_file_that_will_not_parse_is_skipped_rather_than_failing_the_list() {
        let temp = Temp::new("broken");
        let store = temp.store();
        let mut saved = record("good", "fine");
        store.save(&mut saved);
        std::fs::write(temp.0.join("bad.json"), "{ not json").unwrap();
        std::fs::write(temp.0.join("notes.txt"), "ignored").unwrap();

        let listed = store.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "good");
    }

    #[test]
    fn an_id_cannot_put_a_conversation_somewhere_else() {
        let temp = Temp::new("escape");
        let store = temp.store();
        let mut saved = record("../../escape", "x");
        store.save(&mut saved);
        assert!(temp.0.join("_.._escape.json").exists());
    }

    #[test]
    fn a_title_is_flattened_and_cut() {
        assert_eq!(
            title_for("  restart   nginx\nplease "),
            "restart nginx please"
        );
        let long = "x".repeat(80);
        let title = title_for(&long);
        assert_eq!(title.chars().count(), MAX_TITLE + 1);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn the_tab_cuts_the_conversation_and_never_the_machine() {
        assert_eq!(panel_title("short", "web-1", "New chat"), "short (web-1)");
        assert_eq!(panel_title("   ", "web-1", "New chat"), "New chat (web-1)");
        let long = "y".repeat(40);
        let tab = panel_title(&long, "web-1", "New chat");
        assert!(tab.ends_with("… (web-1)"));
    }

    #[test]
    fn usage_is_left_out_when_nothing_reported_any() {
        let json = serde_json::to_value(record("a", "t")).unwrap();
        assert!(json.get("usage").is_none());

        let usage = ChatUsage {
            prompt: 10,
            completion: 2,
            requests: 1,
        };
        let json = serde_json::to_value(usage).unwrap();
        assert_eq!(json["prompt"], 10);
        assert_eq!(json.as_object().unwrap().len(), 3);
    }
}
