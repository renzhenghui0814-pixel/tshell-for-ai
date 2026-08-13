//! Where conversations live between sessions.
//!
//! Two things are kept, because reloading a conversation has to satisfy two
//! different readers. `entries` is what the panel drew -- the turns, the command
//! cards, the output, the file changes -- and replaying it puts the thread back
//! on screen exactly as it was. `messages` is what the model was told, and
//! restoring it is what lets the next message continue the task rather than start
//! a new one.
//!
//! One file per conversation, next to the config file. They hold real command
//! output from real machines, so they are somewhere the user can find, read and
//! delete rather than inside an opaque key-value store.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ai::llm::ChatMessage;
use crate::ai::types::AgentEvent;
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
    /// For everything the assistant did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<AgentEvent>,
}

impl ChatEntry {
    pub fn user(text: impl Into<String>) -> Self {
        Self { kind: "user".into(), text: Some(text.into()), event: None }
    }
    pub fn event(event: AgentEvent) -> Self {
        Self { kind: "event".into(), text: None, event: Some(event) }
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
    /// True once any part of the total was this client's arithmetic rather than
    /// the endpoint's. It never clears: a total that is half measured and half
    /// guessed is a guess, and saying otherwise would be the one dishonest way to
    /// show it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub estimated: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRecord {
    pub id: String,
    pub server_id: String,
    pub server_name: String,
    /// The opening message, shortened. Written once and then left alone.
    pub title: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub entries: Vec<ChatEntry>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ChatUsage>,
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
    let named = if chat_title.trim().is_empty() { fallback } else { chat_title.trim() };
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
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
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
        let Ok(body) = serde_json::to_string(record) else { return };
        // A conversation that cannot be saved is not a reason to interrupt one
        // that is being had. The panel keeps working; the history is what suffers.
        let _ = atomic::write(&self.file_for(&record.id), &body);
    }

    pub fn list(&self) -> Vec<ChatSummary> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return Vec::new() };

        let mut summaries: Vec<ChatSummary> = entries
            .flatten()
            .filter(|entry| {
                entry.path().extension().is_some_and(|extension| extension == "json")
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
        if record.id.is_empty() {
            return None;
        }
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            id: id.into(),
            server_id: "s".into(),
            server_name: "web-1".into(),
            title: title.into(),
            created_at: 1,
            updated_at: 1,
            entries: vec![ChatEntry::user("hello")],
            messages: vec![ChatMessage::user("hello")],
            usage: None,
        }
    }

    #[test]
    fn a_saved_conversation_comes_back_whole() {
        let temp = Temp::new("roundtrip");
        let store = temp.store();
        let mut saved = record("abc", "hello");
        saved.entries.push(ChatEntry::event(AgentEvent::Stopped));
        store.save(&mut saved);

        let loaded = store.load("abc").unwrap();
        assert_eq!(loaded.title, "hello");
        assert_eq!(loaded.entries.len(), 2);
        assert_eq!(loaded.messages.len(), 1);
        assert_eq!(loaded.entries[1].event, Some(AgentEvent::Stopped));
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
        assert_eq!(title_for("  restart   nginx\nplease "), "restart nginx please");
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

        let usage = ChatUsage { prompt: 10, completion: 2, requests: 1, estimated: false };
        let json = serde_json::to_value(usage).unwrap();
        assert_eq!(json["prompt"], 10);
        assert!(json.get("estimated").is_none(), "false is the quiet default");
    }
}
