//! What the assistant knows about a setup before the conversation starts.
//!
//! Two scopes, one file each, plain markdown: `global.md` is true of every
//! machine the user works on, `<serverId>.md` only of one. They live beside the
//! config file, for the same reason it does -- this is the user's own text, and
//! they must be able to open it, read it, correct it and delete it without going
//! through a UI.
//!
//! The user hand-writes into the same file the model appends to, and nothing
//! marks which lines came from where. Distinguishing them would only pay off
//! under an eviction policy that spares one kind, and writing here is refused
//! rather than evicted; all it would buy is a format the user has to be careful
//! not to break.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::atomic;

use crate::ai::types::MemoryScope;

/// Longer than any sane id and short enough to survive every filesystem.
const MAX_ID_LENGTH: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendOutcome {
    Ok,
    Duplicate,
    Full,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Ok,
    Missing,
    Failed,
}

/// Editing one line from the panel. `Missing` is the stale-view answer: the file
/// is no longer what the window drew, so nothing was touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditOutcome {
    Ok,
    Full,
    Missing,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendResult {
    pub outcome: AppendOutcome,
    /// Identifies the line just written, so the panel's undo can find it again.
    pub token: Option<String>,
}

/// Filenames are derived from ids the user typed into `tshell.config.json`, so an
/// id of `../../evil` is something that can actually arrive here. Everything
/// outside a conservative set becomes an underscore and leading dots go, which
/// leaves nothing that can climb out of the directory or name a hidden file.
pub fn memory_file_name(scope: MemoryScope, server_id: Option<&str>) -> String {
    if scope == MemoryScope::Global {
        return "global.md".to_string();
    }
    let cleaned: String = server_id
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_ID_LENGTH)
        .collect();
    let cleaned = cleaned.trim_start_matches('.');
    if cleaned.is_empty() {
        "unnamed.md".to_string()
    } else {
        format!("{cleaned}.md")
    }
}

/// A stored line is `- text`. Comparisons are made on the text, not the bullet.
fn line_text(line: &str) -> String {
    line.trim_start()
        .trim_start_matches(['-', '*'])
        .trim()
        .to_string()
}

/// The bullet or heading a line was written with, so an edit keeps its shape.
fn line_prefix(line: &str) -> String {
    let trimmed = line.trim_start();
    if !trimmed.starts_with(['-', '*']) {
        return String::new();
    }
    let leading = line.len() - trimmed.len();
    let after_marker = &trimmed[1..];
    let spaces = after_marker.len() - after_marker.trim_start().len();
    line[..leading + 1 + spaces].to_string()
}

/// One fact is one line: the whole `forget` contract rests on that being true.
fn flatten(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A token nobody has to guess, made from the clock and a counter.
///
/// It only has to be unique within one window's lifetime -- it names a row in a
/// map that is never written to disk -- so this needs no randomness and no
/// dependency to produce it.
fn make_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0);
    format!("{nanos:x}-{:x}", NEXT.fetch_add(1, Ordering::Relaxed))
}

pub struct MemoryStore {
    dir: PathBuf,
    /// Undo targets, by token. Deliberately not persisted: the button only exists
    /// on a card in the live thread, and a replayed card never offers it. Once the
    /// panel is gone, the way to remove a line is to open the file.
    written: Mutex<HashMap<String, (PathBuf, String)>>,
}

impl MemoryStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            written: Mutex::new(HashMap::new()),
        }
    }

    pub fn file_for(&self, scope: MemoryScope, server_id: Option<&str>) -> PathBuf {
        self.dir.join(memory_file_name(scope, server_id))
    }

    /// The scope's text as it stands, for injecting into the prompt.
    ///
    /// Every failure -- no directory, no file, unreadable file -- is empty
    /// memory. A setup the assistant cannot recall is a worse conversation; one
    /// it cannot have at all is a broken panel.
    pub fn read(&self, scope: MemoryScope, server_id: Option<&str>) -> String {
        std::fs::read_to_string(self.file_for(scope, server_id))
            .map(|text| text.trim().to_string())
            .unwrap_or_default()
    }

    /// The scope's non-empty lines, in file order, exactly as they read.
    ///
    /// For the panel's memory window, which shows every line rather than only the
    /// bullets: someone who put a heading in their own file must be able to see
    /// it there, and a line the window does not draw is a line it could silently
    /// destroy when it rewrites the file around it.
    pub fn lines(&self, scope: MemoryScope, server_id: Option<&str>) -> Vec<String> {
        split_lines(&self.read(scope, server_id))
    }

    /// Replaces one line, identified by where it is and what it says.
    ///
    /// Both, because neither alone is enough. The panel rendered the file at some
    /// earlier moment and the user may have edited it since, so a write by
    /// position alone would land on whatever has moved into that slot. Text alone
    /// is no better: duplicates are legal here, and the user is editing one of
    /// them specifically. So the position is where to look and the text is the
    /// proof it is still the same line -- the rule `edit` already uses on a file.
    pub fn replace(
        &self,
        scope: MemoryScope,
        server_id: Option<&str>,
        index: usize,
        was: &str,
        text: &str,
        budget: usize,
    ) -> EditOutcome {
        let fact = flatten(text);
        if fact.is_empty() {
            return EditOutcome::Failed;
        }

        let file = self.file_for(scope, server_id);
        let mut lines = split_lines(&self.read(scope, server_id));
        // Stale view: the file is not what was drawn, so nothing is touched and
        // the caller redraws from what is actually there.
        if index >= lines.len() || lines[index] != was {
            return EditOutcome::Missing;
        }

        // A bullet stays a bullet and a heading stays a heading: what is edited is
        // the text, not the shape the user gave the line.
        lines[index] = format!("{}{fact}", line_prefix(&lines[index]));
        let body = format!("{}\n", lines.join("\n"));
        if budget > 0 && body.chars().count() > budget {
            return EditOutcome::Full;
        }

        match atomic::write(&file, &body) {
            Ok(()) => EditOutcome::Ok,
            Err(_) => EditOutcome::Failed,
        }
    }

    /// Removes the line at `index`, checked against `was` the same way
    /// [`Self::replace`] is.
    pub fn remove_at(
        &self,
        scope: MemoryScope,
        server_id: Option<&str>,
        index: usize,
        was: &str,
    ) -> RemoveOutcome {
        let file = self.file_for(scope, server_id);
        let mut lines = split_lines(&self.read(scope, server_id));
        if index >= lines.len() || lines[index] != was {
            return RemoveOutcome::Missing;
        }
        lines.remove(index);
        rewrite(&file, &lines)
    }

    /// `budget` is the characters the whole file may reach. The write is refused
    /// whole rather than trimmed: half a fact is worse than no fact, and the model
    /// is told so it can make room itself.
    pub fn append(
        &self,
        scope: MemoryScope,
        server_id: Option<&str>,
        text: &str,
        budget: usize,
    ) -> AppendResult {
        let fact = flatten(text);
        if fact.is_empty() {
            return AppendResult {
                outcome: AppendOutcome::Failed,
                token: None,
            };
        }

        let file = self.file_for(scope, server_id);
        let current = self.read(scope, server_id);
        if current.lines().any(|line| line_text(line) == fact) {
            return AppendResult {
                outcome: AppendOutcome::Duplicate,
                token: None,
            };
        }

        let line = format!("- {fact}");
        let next = if current.is_empty() {
            format!("{line}\n")
        } else {
            format!("{current}\n{line}\n")
        };
        if budget == 0 || next.chars().count() > budget {
            return AppendResult {
                outcome: AppendOutcome::Full,
                token: None,
            };
        }

        if atomic::write(&file, &next).is_err() {
            return AppendResult {
                outcome: AppendOutcome::Failed,
                token: None,
            };
        }
        let token = make_token();
        self.written
            .lock()
            .unwrap()
            .insert(token.clone(), (file, line));
        AppendResult {
            outcome: AppendOutcome::Ok,
            token: Some(token),
        }
    }

    /// Takes back one specific line the assistant wrote.
    ///
    /// By token and not by text, because between the card appearing and the
    /// button being pressed the user may have edited the file: a line that no
    /// longer says what it said is not the line they are undoing, and a second
    /// copy typed by hand is not either. Nothing left to undo is a no-op, never
    /// an error.
    pub fn undo(&self, token: &str) -> RemoveOutcome {
        let Some((file, line)) = self.written.lock().unwrap().remove(token) else {
            return RemoveOutcome::Missing;
        };

        let Ok(current) = std::fs::read_to_string(&file) else {
            return RemoveOutcome::Missing;
        };
        let mut lines: Vec<String> = current.trim().lines().map(str::to_string).collect();
        let Some(index) = lines.iter().rposition(|entry| *entry == line) else {
            return RemoveOutcome::Missing;
        };
        lines.remove(index);
        rewrite(&file, &lines)
    }
}

fn split_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect()
}

fn rewrite(file: &Path, lines: &[String]) -> RemoveOutcome {
    let body = lines.join("\n").trim().to_string();
    let content = if body.is_empty() {
        String::new()
    } else {
        format!("{body}\n")
    };
    match atomic::write(file, &content) {
        Ok(()) => RemoveOutcome::Ok,
        Err(_) => RemoveOutcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("tshell-memory-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> MemoryStore {
            MemoryStore::new(&self.0)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const BUDGET: usize = 2000;

    #[test]
    fn a_server_id_cannot_climb_out_of_the_directory() {
        assert_eq!(
            memory_file_name(MemoryScope::Global, Some("anything")),
            "global.md"
        );
        assert_eq!(
            memory_file_name(MemoryScope::Server, Some("../../evil")),
            "_.._evil.md"
        );
        assert_eq!(
            memory_file_name(MemoryScope::Server, Some("...")),
            "unnamed.md"
        );
        assert_eq!(memory_file_name(MemoryScope::Server, None), "unnamed.md");
        assert_eq!(
            memory_file_name(MemoryScope::Server, Some("web-1.prod")),
            "web-1.prod.md"
        );
    }

    #[test]
    fn a_fact_is_appended_as_one_flattened_line() {
        let temp = Temp::new("append");
        let store = temp.store();
        let result = store.append(
            MemoryScope::Server,
            Some("s"),
            "  nginx  lives\n at /opt  ",
            BUDGET,
        );
        assert_eq!(result.outcome, AppendOutcome::Ok);
        assert!(result.token.is_some());
        assert_eq!(
            store.read(MemoryScope::Server, Some("s")),
            "- nginx lives at /opt"
        );
    }

    #[test]
    fn the_same_fact_twice_is_not_written_twice() {
        let temp = Temp::new("dup");
        let store = temp.store();
        store.append(MemoryScope::Server, Some("s"), "a fact", BUDGET);
        let again = store.append(MemoryScope::Server, Some("s"), "a fact", BUDGET);
        assert_eq!(again.outcome, AppendOutcome::Duplicate);
        assert_eq!(store.lines(MemoryScope::Server, Some("s")).len(), 1);
    }

    #[test]
    fn a_full_scope_refuses_the_whole_write() {
        let temp = Temp::new("full");
        let store = temp.store();
        let result = store.append(MemoryScope::Server, Some("s"), "a fact", 5);
        assert_eq!(result.outcome, AppendOutcome::Full);
        assert_eq!(store.read(MemoryScope::Server, Some("s")), "");

        let zero = store.append(MemoryScope::Server, Some("s"), "a fact", 0);
        assert_eq!(zero.outcome, AppendOutcome::Full);
    }

    #[test]
    fn an_empty_fact_is_not_a_fact() {
        let temp = Temp::new("empty");
        let store = temp.store();
        assert_eq!(
            store
                .append(MemoryScope::Server, Some("s"), "   ", BUDGET)
                .outcome,
            AppendOutcome::Failed
        );
    }

    #[test]
    fn undo_takes_back_the_line_it_wrote_and_only_that_one() {
        let temp = Temp::new("undo");
        let store = temp.store();
        let first = store
            .append(MemoryScope::Global, None, "one", BUDGET)
            .token
            .unwrap();
        store.append(MemoryScope::Global, None, "two", BUDGET);

        assert_eq!(store.undo(&first), RemoveOutcome::Ok);
        assert_eq!(store.lines(MemoryScope::Global, None), vec!["- two"]);
        // A token is spent once; pressing undo twice is a no-op, not an error.
        assert_eq!(store.undo(&first), RemoveOutcome::Missing);
        assert_eq!(store.undo("never-issued"), RemoveOutcome::Missing);
    }

    #[test]
    fn an_edit_keeps_the_shape_of_the_line() {
        let temp = Temp::new("edit");
        let store = temp.store();
        std::fs::write(temp.0.join("global.md"), "## Heading\n- a bullet\n").unwrap();

        assert_eq!(
            store.replace(
                MemoryScope::Global,
                None,
                1,
                "- a bullet",
                "a better bullet",
                BUDGET
            ),
            EditOutcome::Ok
        );
        assert_eq!(
            store.lines(MemoryScope::Global, None)[1],
            "- a better bullet"
        );

        assert_eq!(
            store.replace(
                MemoryScope::Global,
                None,
                0,
                "## Heading",
                "New Heading",
                BUDGET
            ),
            EditOutcome::Ok
        );
        assert_eq!(store.lines(MemoryScope::Global, None)[0], "New Heading");
    }

    #[test]
    fn a_stale_view_touches_nothing() {
        let temp = Temp::new("stale");
        let store = temp.store();
        store.append(MemoryScope::Global, None, "one", BUDGET);

        assert_eq!(
            store.replace(
                MemoryScope::Global,
                None,
                0,
                "- something else",
                "x",
                BUDGET
            ),
            EditOutcome::Missing
        );
        assert_eq!(
            store.replace(MemoryScope::Global, None, 9, "- one", "x", BUDGET),
            EditOutcome::Missing
        );
        assert_eq!(
            store.remove_at(MemoryScope::Global, None, 0, "- wrong"),
            RemoveOutcome::Missing
        );
        assert_eq!(store.lines(MemoryScope::Global, None), vec!["- one"]);
    }

    #[test]
    fn removing_the_last_line_leaves_an_empty_file_rather_than_a_stray_newline() {
        let temp = Temp::new("last");
        let store = temp.store();
        store.append(MemoryScope::Global, None, "only", BUDGET);
        assert_eq!(
            store.remove_at(MemoryScope::Global, None, 0, "- only"),
            RemoveOutcome::Ok
        );
        assert_eq!(store.read(MemoryScope::Global, None), "");
    }

    #[test]
    fn a_scope_that_was_never_written_reads_as_empty() {
        let temp = Temp::new("absent");
        let store = temp.store();
        assert_eq!(store.read(MemoryScope::Server, Some("nobody")), "");
        assert!(store.lines(MemoryScope::Server, Some("nobody")).is_empty());
    }

    #[test]
    fn the_two_scopes_are_separate_files() {
        let temp = Temp::new("scopes");
        let store = temp.store();
        store.append(MemoryScope::Global, None, "everywhere", BUDGET);
        store.append(MemoryScope::Server, Some("s"), "here only", BUDGET);
        assert_eq!(store.read(MemoryScope::Global, None), "- everywhere");
        assert_eq!(store.read(MemoryScope::Server, Some("s")), "- here only");
    }
}
