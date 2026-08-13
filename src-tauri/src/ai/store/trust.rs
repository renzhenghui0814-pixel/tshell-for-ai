//! The directories the user has stopped being asked about.
//!
//! `policy::path` draws the red line and `policy::command` guards what may run.
//! This is neither: it holds a list of places the user has already said yes to,
//! so the three file actions can land there without a dialog. It can only ever
//! turn a `confirm` into a silent write -- a refusal stays a refusal, in every
//! mode, and nothing here is consulted until that check has already passed.
//!
//! Kept per server, because a path is not a place. `/home/trade/work` on the box
//! you are developing on and the same string on a production machine name two
//! different directories, and trusting one must never quietly trust the other.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ai::policy::path::normalize_path;
use crate::atomic;

const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustedFile {
    pub version: u32,
    /// Ordered so the file reads the same twice running, which matters for
    /// something users diff and hand-edit.
    pub servers: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustWrite {
    /// Written.
    Ok,
    /// The list already said what was asked for, so nothing was written: an add
    /// of something already covered, or a remove of something already gone.
    NoChange,
    /// Not an absolute path, so there is no directory this could mean.
    Relative,
    /// The file is on disk but is not readable as JSON, so it was left alone.
    Unreadable,
}

/// The parent directory of a path, textually.
fn parent_of(target: &str) -> String {
    match target.rfind('/') {
        None => String::new(),
        Some(0) => "/".to_string(),
        Some(at) => target[..at].to_string(),
    }
}

/// Whether `dir` is `root` or sits underneath it.
///
/// The trailing slash is the whole point: without it `/home/x` would swallow
/// `/home/xyz`, which is a different directory that nobody trusted. `/` is the
/// one root that cannot be written that way, so it answers for every absolute
/// path -- which is what trusting `/` means, and why it is only ever reachable by
/// editing the file rather than by clicking anything.
fn covers(root: &str, dir: &str) -> bool {
    if root == "/" {
        return dir.starts_with('/');
    }
    dir == root || dir.starts_with(&format!("{root}/"))
}

pub struct TrustStore {
    file: PathBuf,
}

impl TrustStore {
    pub fn new(file: impl Into<PathBuf>) -> Self {
        Self { file: file.into() }
    }

    pub fn path(&self) -> &PathBuf {
        &self.file
    }

    /// The file as it stands, or `None` when it is there but unreadable.
    ///
    /// A missing file is an empty list rather than a failure: nobody has trusted
    /// anything yet, which is the state everyone starts in. A file that will not
    /// parse is different -- it is someone's hand edit, half finished -- so it
    /// comes back as `None` and the writers refuse rather than flattening it.
    fn load(&self) -> Option<TrustedFile> {
        let empty =
            || TrustedFile { version: CURRENT_VERSION, servers: BTreeMap::new() };
        let Ok(text) = std::fs::read_to_string(&self.file) else { return Some(empty()) };
        if text.trim().is_empty() {
            return Some(empty());
        }

        let parsed: serde_json::Value = serde_json::from_str(&text).ok()?;
        let object = parsed.as_object()?;
        let mut servers = BTreeMap::new();
        if let Some(listed) = object.get("servers").and_then(|value| value.as_object()) {
            for (id, list) in listed {
                let Some(list) = list.as_array() else { continue };
                servers.insert(
                    id.clone(),
                    list.iter()
                        .filter_map(|entry| entry.as_str())
                        .filter(|entry| entry.trim().starts_with('/'))
                        .map(normalize_path)
                        .collect(),
                );
            }
        }
        Some(TrustedFile { version: CURRENT_VERSION, servers })
    }

    fn save(&self, data: &TrustedFile) -> std::io::Result<()> {
        let body = format!("{}\n", serde_json::to_string_pretty(data)?);
        atomic::write(&self.file, &body)
    }

    /// One server's directories, in the order they were trusted. Never fails.
    pub fn list(&self, server_id: &str) -> Vec<String> {
        self.load()
            .and_then(|data| data.servers.get(server_id).cloned())
            .unwrap_or_default()
    }

    /// Whether a file action on `target` may go ahead without asking.
    ///
    /// The directory holding the file is what is compared, not the file: trusting
    /// a directory is a statement about everything in it and everything below it.
    ///
    /// A relative path is never trusted. Where it lands depends on where the
    /// shell is standing, which this cannot know and must not guess -- and a
    /// wrong guess here writes a file with nobody watching.
    pub fn is_trusted(&self, server_id: &str, target: &str) -> bool {
        if !target.trim().starts_with('/') {
            return false;
        }
        let dir = parent_of(&normalize_path(target));
        if dir.is_empty() {
            return false;
        }
        self.list(server_id).iter().any(|root| covers(root, &dir))
    }

    /// Trusts a directory, and drops anything it already covers.
    ///
    /// Trusting `/srv/app` when `/srv/app/conf` is on the list leaves the second
    /// one saying nothing, and a list that keeps entries which no longer decide
    /// anything is a list nobody can read. The broader entry wins and the
    /// narrower ones go.
    pub fn add(&self, server_id: &str, dir: &str) -> TrustWrite {
        if !dir.trim().starts_with('/') {
            return TrustWrite::Relative;
        }
        let root = normalize_path(dir);
        let Some(mut data) = self.load() else { return TrustWrite::Unreadable };

        let current = data.servers.get(server_id).cloned().unwrap_or_default();
        if current.iter().any(|entry| covers(entry, &root)) {
            return TrustWrite::NoChange;
        }

        let mut kept: Vec<String> =
            current.into_iter().filter(|entry| !covers(&root, entry)).collect();
        kept.push(root);
        data.servers.insert(server_id.to_string(), kept);
        match self.save(&data) {
            Ok(()) => TrustWrite::Ok,
            Err(_) => TrustWrite::Unreadable,
        }
    }

    /// Drops one entry, matched exactly as it reads in the list.
    pub fn remove(&self, server_id: &str, dir: &str) -> TrustWrite {
        let Some(mut data) = self.load() else { return TrustWrite::Unreadable };

        let current = data.servers.get(server_id).cloned().unwrap_or_default();
        let kept: Vec<String> = current.iter().filter(|entry| *entry != dir).cloned().collect();
        if kept.len() == current.len() {
            return TrustWrite::NoChange;
        }

        data.servers.insert(server_id.to_string(), kept);
        match self.save(&data) {
            Ok(()) => TrustWrite::Ok,
            Err(_) => TrustWrite::Unreadable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("tshell-trust-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> TrustStore {
            TrustStore::new(self.0.join("trusted.json"))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn nothing_is_trusted_before_anything_is_added() {
        let temp = Temp::new("fresh");
        let store = temp.store();
        assert!(store.list("s").is_empty());
        assert!(!store.is_trusted("s", "/srv/app/x.conf"));
    }

    #[test]
    fn trusting_a_directory_covers_what_is_under_it() {
        let temp = Temp::new("covers");
        let store = temp.store();
        assert_eq!(store.add("s", "/srv/app"), TrustWrite::Ok);
        assert!(store.is_trusted("s", "/srv/app/x.conf"));
        assert!(store.is_trusted("s", "/srv/app/deep/down/y.conf"));
        assert!(!store.is_trusted("s", "/srv/other/x.conf"));
    }

    #[test]
    fn a_neighbour_with_the_same_prefix_is_not_covered() {
        let temp = Temp::new("neighbour");
        let store = temp.store();
        store.add("s", "/home/x");
        assert!(store.is_trusted("s", "/home/x/file"));
        assert!(!store.is_trusted("s", "/home/xyz/file"));
    }

    #[test]
    fn trust_is_per_server() {
        let temp = Temp::new("perserver");
        let store = temp.store();
        store.add("dev", "/srv/app");
        assert!(store.is_trusted("dev", "/srv/app/x"));
        assert!(!store.is_trusted("prod", "/srv/app/x"));
    }

    #[test]
    fn a_relative_path_is_never_trusted() {
        let temp = Temp::new("relative");
        let store = temp.store();
        store.add("s", "/");
        assert_eq!(store.add("s", "relative/dir"), TrustWrite::Relative);
        assert!(!store.is_trusted("s", "relative/file"));
        // Trusting `/` does answer for every absolute path, which is what it means.
        assert!(store.is_trusted("s", "/anywhere/at/all"));
    }

    #[test]
    fn a_broader_entry_replaces_the_narrower_ones_it_covers() {
        let temp = Temp::new("broader");
        let store = temp.store();
        store.add("s", "/srv/app/conf");
        store.add("s", "/srv/app/logs");
        assert_eq!(store.list("s").len(), 2);

        assert_eq!(store.add("s", "/srv/app"), TrustWrite::Ok);
        assert_eq!(store.list("s"), vec!["/srv/app"]);
    }

    #[test]
    fn adding_something_already_covered_writes_nothing() {
        let temp = Temp::new("nochange");
        let store = temp.store();
        store.add("s", "/srv/app");
        assert_eq!(store.add("s", "/srv/app/conf"), TrustWrite::NoChange);
        assert_eq!(store.add("s", "/srv/app/"), TrustWrite::NoChange);
        assert_eq!(store.list("s"), vec!["/srv/app"]);
    }

    #[test]
    fn removing_takes_the_entry_exactly_as_it_reads() {
        let temp = Temp::new("remove");
        let store = temp.store();
        store.add("s", "/srv/app");
        assert_eq!(store.remove("s", "/srv/other"), TrustWrite::NoChange);
        assert_eq!(store.remove("s", "/srv/app"), TrustWrite::Ok);
        assert!(store.list("s").is_empty());
    }

    #[test]
    fn a_hand_edit_that_will_not_parse_is_left_alone() {
        let temp = Temp::new("broken");
        let store = temp.store();
        std::fs::write(store.path(), "{ not json at all").unwrap();
        assert_eq!(store.add("s", "/srv/app"), TrustWrite::Unreadable);
        assert_eq!(store.remove("s", "/srv/app"), TrustWrite::Unreadable);
        assert!(store.list("s").is_empty());
        // And still on disk, exactly as the user left it.
        assert_eq!(std::fs::read_to_string(store.path()).unwrap(), "{ not json at all");
    }

    #[test]
    fn dot_dot_in_the_target_is_resolved_before_it_is_compared() {
        let temp = Temp::new("dotdot");
        let store = temp.store();
        store.add("s", "/srv/app");
        assert!(store.is_trusted("s", "/srv/app/sub/../x.conf"));
        assert!(!store.is_trusted("s", "/srv/app/../other/x.conf"));
    }
}
