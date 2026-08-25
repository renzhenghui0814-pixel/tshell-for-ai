//! A verbatim record of what passed between this client and the model.
//!
//! It exists because the failures worth chasing here are all invisible from the
//! panel. A reply that would not parse is shown as a nudge and a second attempt,
//! with the malformed text nowhere; an answer that arrives all at once looks the
//! same whether the endpoint streamed it or handed it over in one piece; a step
//! that ran the wrong action looks exactly like a step that ran the right one.
//! Every one of those is obvious the moment the exchange is on disk, and
//! essentially undiagnosable from a screenshot.
//!
//! Three things follow from that purpose, and they are the whole design:
//!
//! It is NOT masked. `policy::redact` exists for what leaves the machine, and
//! this never leaves it -- masking here would edit exactly the characters a parse
//! failure turns on, which is the one thing that must not happen to evidence.
//!
//! It is off unless asked for, and it says so in the file it writes. Something
//! that records command output and memory verbatim is a thing the user switches
//! on to look into a problem, not a thing that quietly accumulates.
//!
//! It never breaks a task. Every failure is swallowed: a log that cannot be
//! written is worth nothing at all next to the conversation it was describing.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

/// One request, as close to the bytes on the wire as is useful to read.
pub struct LogRequest<'a> {
    /// The URL actually posted to, `/chat/completions` and all.
    pub endpoint: &'a str,
    /// The body as it goes out, minus `messages`: model, temperature, stream, and
    /// whatever thinking fields this request carried. The API key is not in here
    /// and never reaches this file -- it travels as a header and is not logged.
    pub params: &'a Value,
    pub system: &'a str,
    pub messages: Vec<(String, String)>,
}

#[derive(Default)]
struct Progress {
    /// The system prompt as last written. It changes with memory, skills, mode.
    logged_system: String,
    /// How much of the conversation is already in the file.
    logged: usize,
}

/// What a file will be called and what goes at the top of it, held until there
/// is something to put in it.
struct Plan {
    base: String,
    header: String,
    keep: usize,
}

/// Where one panel's exchange is written, or nowhere when logging is off.
///
/// # The file is not created until something is written to it
///
/// A panel opens whenever anyone looks at the assistant, and most of those
/// panels are never spoken to. Creating the file up front left a transcript per
/// glance -- and because pruning keeps the last N files, a handful of glances
/// evicted the transcript of the conversation someone was trying to keep. What
/// starts a transcript is a request, so a request is what creates the file.
pub struct LogSession {
    /// The store, so the file can still be made later. `None` when logging is
    /// off, which is what `active` reads.
    plan: Option<(LogStore, Plan)>,
    file: Mutex<Option<PathBuf>>,
    progress: Mutex<Progress>,
}

impl LogSession {
    /// The session handed out when logging is off. Costs a branch per call.
    pub fn inactive() -> Self {
        Self {
            plan: None,
            file: Mutex::new(None),
            progress: Mutex::new(Progress::default()),
        }
    }

    /// Whether anything would be written, not whether anything has been.
    pub fn active(&self) -> bool {
        self.plan.is_some()
    }

    /// Where it is being written, once it is. `None` before the first record --
    /// which is the honest answer, because until then there is no file.
    #[cfg(test)]
    pub fn file(&self) -> Option<PathBuf> {
        self.file.lock().unwrap().clone()
    }

    /*
     * The file, making it if this is the first record.
     *
     * Pruning happens here rather than at `open` for the same reason the
     * creation does: what it counts should be transcripts, and until this point
     * there was no transcript to count this one among.
     */
    fn ensure(&self) -> Option<PathBuf> {
        let plan = self.plan.as_ref()?;
        let mut file = self.file.lock().unwrap();
        if let Some(path) = file.as_ref() {
            return Some(path.clone());
        }
        if std::fs::create_dir_all(&plan.0.dir).is_err() {
            return None;
        }
        // Before this file exists, so it is never a candidate for its own sweep.
        plan.0.prune(plan.1.keep.saturating_sub(1));
        let made = plan.0.create(&plan.1.base, &plan.1.header);
        *file = made.clone();
        made
    }

    /// What was added to the conversation this time, not the whole of it.
    ///
    /// A task of twenty steps sends the entire history on every one of them, so a
    /// file that recorded each request in full would hold the same command output
    /// twenty times and be unreadable by the third. What is new is what is worth
    /// having; the rest is already above it in the file.
    pub fn request(&self, info: &LogRequest) {
        if !self.active() {
            return;
        }
        let mut parts: Vec<String> = Vec::new();
        let mut progress = self.progress.lock().unwrap();

        /*
         * Only when it has changed, which is not never: memory, the skill
         * manifest and the thinking switch all rewrite it between steps. Written
         * whole when it does, because a diff of a prompt is harder to read than
         * the prompt.
         */
        if info.system != progress.logged_system {
            progress.logged_system = info.system.to_string();
            parts.push(format!("[system]\n{}", info.system));
        }
        // A new conversation starts the array over, and the count with it.
        if info.messages.len() < progress.logged {
            progress.logged = 0;
        }
        for (role, content) in info.messages.iter().skip(progress.logged) {
            // The model's own turns are in this file already, as `response`.
            // Writing them again here would double every answer in the transcript.
            if role == "assistant" {
                continue;
            }
            parts.push(format!("[{role}]\n{content}"));
        }
        progress.logged = info.messages.len();
        drop(progress);

        let mut head = vec![format!("endpoint:{}", info.endpoint)];
        if let Some(params) = info.params.as_object() {
            for (key, value) in params {
                head.push(format!("{key}:{}", plain(value)));
            }
        }
        self.append("request", &head.join(" "), &parts.join("\n\n"));
    }

    /// The answer, once, whole. Never per chunk: that is thousands of lines
    /// saying nothing.
    pub fn response(&self, text: &str) {
        self.append("response", "", text);
    }

    /// The thinking, once, whole, and only when there was some.
    pub fn reasoning(&self, text: &str) {
        self.append("reasoning", "", text);
    }

    /// One record: a header line that starts with the time and the type, then the
    /// text itself, then a blank line.
    ///
    /// The text goes in verbatim, newlines and all. Escaping it onto one line
    /// would make the file greppable and unreadable, and the reason anyone opens
    /// this is to read a prompt or an answer that did not do what they expected.
    fn append(&self, kind: &str, head: &str, content: &str) {
        let Some(file) = self.ensure() else { return };
        let spaced = if head.is_empty() {
            String::new()
        } else {
            format!(" {head}")
        };
        let record = format!(
            "time:{} type:{kind}{spaced} content:{content}\n\n",
            stamp_ms()
        );
        // Opened per record rather than held: the write is rare, and a handle
        // kept open is a handle to close on every path out of a panel.
        let written = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)
            .and_then(|mut handle| handle.write_all(record.as_bytes()));
        let _ = written;
    }
}

/// A parameter as it would read in the body: bare for a scalar, JSON for the rest.
fn plain(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Object(_) | Value::Array(_) => value.to_string(),
        other => other.to_string(),
    }
}

/// `20260709-12:00:00.212`, in local time, to the millisecond.
fn stamp_ms() -> String {
    chrono::Local::now()
        .format("%Y%m%d-%H:%M:%S%.3f")
        .to_string()
}

/// `2026-07-09_12-00-00`, for a filename.
///
/// Local time, not UTC: the user is looking for the file from when they saw it.
fn stamp_file() -> String {
    chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

/// What a filename may not contain on the strictest platform this runs on.
const RESERVED: &[char] = &['<', '>', ':', '"', '/', '?', '*', '|', '\\'];

/// A server name as a filename, keeping as much of it as is safe to keep.
///
/// Deliberately not an ASCII allowlist. A user whose machines are all named in
/// Chinese would get a directory of files called `_____.log`, told apart only by
/// their timestamps -- which defeats the one reason the name is in the filename
/// at all. Every filesystem this runs on takes UTF-8, so what goes is what is
/// genuinely unusable rather than what is merely not English: the
/// Windows-reserved set, both separators, whitespace, and control characters.
///
/// Leading dots and underscores go, so a name can neither produce a hidden file
/// nor climb out of the directory.
fn slug(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|char| {
            let unusable = (char as u32) < 0x20
                || char as u32 == 0x7f
                || RESERVED.contains(&char)
                || char.is_whitespace();
            if unusable {
                '_'
            } else {
                char
            }
        })
        .collect();
    let trimmed: String = mapped
        .trim_start_matches(['.', '_'])
        .chars()
        .take(40)
        .collect();
    let trimmed = trimmed.trim_end_matches('.');
    if trimmed.is_empty() {
        "server".to_string()
    } else {
        trimmed.to_string()
    }
}

/// One transcript on disk, as the panel lists it.
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    /// The filename, which carries the machine, the conversation and the time.
    pub name: String,
    /// The full path, which is what opening it needs.
    pub path: String,
    pub size: u64,
    /// Seconds since the epoch, or zero when it cannot be read.
    pub modified: u64,
}

#[derive(Clone)]
pub struct LogStore {
    dir: PathBuf,
}

impl LogStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Opens a file for one chat panel, or hands back the inactive session.
    ///
    /// Named for the machine, the conversation, and when it started.
    ///
    /// # One file per conversation, not per panel
    ///
    /// A panel outlives its conversations: "new chat" starts a fresh one in the
    /// same tab, and so does loading an old one. Keeping one file per panel put
    /// all of them end to end, which reads as a single exchange in which the
    /// model periodically forgets everything and re-reads its prompt.
    ///
    /// `chat_id` is the conversation's own id -- the same one the chat store
    /// files it under -- so a transcript and the conversation it describes can be
    /// put side by side without guessing from timestamps.
    pub fn open(&self, enabled: bool, keep: usize, server_name: &str, chat_id: &str) -> LogSession {
        self.open_with_progress(enabled, keep, server_name, chat_id, None)
    }

    /// Reopens a conversation's existing transcript, if it has one. The count
    /// comes from its durable chat history, so the next request appends only the
    /// new turn rather than rewriting the context already present in the file.
    pub fn resume(
        &self,
        enabled: bool,
        keep: usize,
        server_name: &str,
        chat_id: &str,
        logged_messages: usize,
    ) -> LogSession {
        self.open_with_progress(enabled, keep, server_name, chat_id, Some(logged_messages))
    }

    fn open_with_progress(
        &self,
        enabled: bool,
        keep: usize,
        server_name: &str,
        chat_id: &str,
        resume_messages: Option<usize>,
    ) -> LogSession {
        if !enabled {
            return LogSession::inactive();
        }

        // A header rather than a record: it is about the file, not about the
        // conversation, and a reader scanning for `time:` should not trip over it.
        let header = format!(
            "# tshell AI transcript -- {server_name}\n\
             # Recorded verbatim and NOT masked: it holds command output, memory and anything\n\
             # else the model was sent. The API key is not in here. Delete it when you are done.\n\n"
        );

        // A log that cannot be opened is not a reason to refuse to hold the
        // conversation it was going to describe.
        // Machine first, then the conversation, then when: the machine is what
        // someone scanning the directory is looking for, and the id is what they
        // match against a chat they still have open.
        let base = format!("{}_{}_{}", slug(server_name), slug(chat_id), stamp_file());
        let resumed = resume_messages.and_then(|logged| {
            self.latest_for_chat(server_name, chat_id)
                .map(|file| (file, logged))
        });
        LogSession {
            plan: Some((self.clone(), Plan { base, header, keep })),
            file: Mutex::new(resumed.as_ref().map(|(file, _)| file.clone())),
            progress: Mutex::new(Progress {
                logged_system: String::new(),
                logged: resumed.map(|(_, logged)| logged).unwrap_or(0),
            }),
        }
    }

    /// Finds the newest transcript for exactly this server/chat pair. Older
    /// versions could create more than one while reopening a chat; choosing the
    /// newest preserves continuity without merging unrelated files.
    fn latest_for_chat(&self, server_name: &str, chat_id: &str) -> Option<PathBuf> {
        let prefix = format!("{}_{}_", slug(server_name), slug(chat_id));
        std::fs::read_dir(&self.dir)
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "log")
                    && path
                        .file_name()
                        .is_some_and(|name| name.to_string_lossy().starts_with(&prefix))
            })
            .max_by_key(|path| {
                std::fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .ok()
            })
    }

    /// Creates a file nobody else is writing to, and puts the header in it.
    ///
    /// # Why this is not `fs::write`
    ///
    /// It was, and the name is only accurate to the second. Two panels opened
    /// within one second -- two assistants on one server, which is a normal
    /// thing to do -- landed on the same name, and `fs::write` truncates: the
    /// second panel wiped the first one's file, and from then on both appended
    /// to it with a `Progress` apiece. What that produces is a transcript that
    /// looks corrupt in a very specific and misleading way. Each panel writes
    /// the system prompt again, and the whole conversation again, the first time
    /// it says anything -- because as far as ITS progress is concerned nothing
    /// has been written yet. Reading it back, the model appears to be resending
    /// its entire context and re-reading a prompt that never changed.
    ///
    /// `create_new` is the whole fix: it fails rather than truncates, so a name
    /// already taken sends this round the loop for the next one.
    fn create(&self, base: &str, header: &str) -> Option<PathBuf> {
        use std::io::Write;

        // Far more than could ever collide -- a second holds a handful of panel
        // openings -- and bounded so a directory that refuses every write ends
        // the search rather than spinning in it.
        for attempt in 0..64 {
            let name = if attempt == 0 {
                format!("{base}.log")
            } else {
                format!("{base}-{}.log", attempt + 1)
            };
            let path = self.dir.join(name);
            match std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&path)
            {
                Ok(mut handle) => {
                    return handle.write_all(header.as_bytes()).ok().map(|()| path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                // Not a name that is taken: a directory that is gone, or one this
                // user may not write to. The next name would fail the same way.
                Err(_) => return None,
            }
        }
        None
    }

    /// Drops the oldest files past `keep`.
    ///
    /// By modification time, not by name. The name used to start with the stamp,
    /// so sorting it sorted by age; it now starts with the machine, so sorting it
    /// would keep whichever servers come last in the alphabet and delete the rest
    /// however recent they were. Time is what "oldest" meant all along, and it is
    /// the one thing that stays true whatever the name is made of.
    /// One machine's transcripts, newest first.
    ///
    /// Filtered here rather than in the page because the filename convention is
    /// this module's -- the page would have to know that a name is slugged and
    /// that the machine comes first, which is exactly the kind of knowledge that
    /// drifts once it lives in two places.
    ///
    /// Ordered by time and then by name. Time is what "newest" means, but two
    /// panels opened in the same second share it, and a list whose order changes
    /// between two identical calls is a list the eye cannot keep its place in.
    pub fn list(&self, server_name: &str) -> Vec<LogFile> {
        let prefix = format!("{}_", slug(server_name));
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };

        let mut files: Vec<LogFile> = entries
            .flatten()
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "log")
            })
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with(&prefix) {
                    return None;
                }
                let meta = entry.metadata().ok();
                Some(LogFile {
                    path: entry.path().display().to_string(),
                    name,
                    size: meta.as_ref().map(|meta| meta.len()).unwrap_or(0),
                    modified: meta
                        .and_then(|meta| meta.modified().ok())
                        .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|since| since.as_secs())
                        .unwrap_or(0),
                })
            })
            .collect();

        files.sort_by(|left, right| {
            right
                .modified
                .cmp(&left.modified)
                .then_with(|| right.name.cmp(&left.name))
        });
        files
    }

    /// Delete one transcript, if it is one of ours.
    ///
    /// Three conditions, and none of them is "the page said so": the path has to
    /// resolve inside this directory, it has to end in `.log`, and it has to
    /// exist. `canonicalize` is what makes the first one mean anything -- it
    /// resolves `..` and any symlink before the comparison, so a path that
    /// climbs out and points back at something else is refused for where it
    /// lands rather than accepted for how it reads.
    ///
    /// False for anything refused or already gone. There is nothing for the
    /// panel to do about either, and both end with the file not being there.
    pub fn remove(&self, path: &str) -> bool {
        let Ok(target) = std::fs::canonicalize(path) else {
            return false;
        };
        let Ok(dir) = std::fs::canonicalize(&self.dir) else {
            return false;
        };
        if target.parent() != Some(dir.as_path()) {
            return false;
        }
        if target
            .extension()
            .is_none_or(|extension| extension != "log")
        {
            return false;
        }
        std::fs::remove_file(&target).is_ok()
    }

    fn prune(&self, keep: usize) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
            .collect();
        // A file whose time cannot be read sorts oldest, so it is the first to go
        // rather than something that survives every sweep by being unreadable.
        names.sort_by_key(|path| {
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
        });
        let over = names.len().saturating_sub(keep);
        for path in names.into_iter().take(over) {
            // One file that will not go is not a reason to keep none of the rest.
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("tshell-log-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn store(&self) -> LogStore {
            LogStore::new(&self.0)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The list is one machine's transcripts, not the directory.
    ///
    /// The button that opens it sits in a chat panel, which is attached to one
    /// server; handing it every log on the machine would mean the user picking
    /// their own server out of a list of all of them, every time, in a panel
    /// that already knows which one it is.
    #[test]
    fn the_listing_is_filtered_to_one_machine() {
        let temp = Temp::new("list-filter");
        let store = temp.store();
        // Written to, because a transcript that was never spoken to has no file
        // and so is not in any listing -- which is the point of it.
        for (host, id) in [
            ("10.0.1.168", "chat-a"),
            ("10.0.1.168", "chat-b"),
            ("10.0.1.216", "chat-c"),
        ] {
            store.open(true, 9, host, id).response("anything");
        }

        let mine = store.list("10.0.1.168");
        assert_eq!(mine.len(), 2);
        assert!(
            mine.iter().all(|file| file.name.starts_with("10.0.1.168_")),
            "{mine:?}"
        );
        assert_eq!(store.list("10.0.1.216").len(), 1);
        assert_eq!(store.list("10.0.1.99").len(), 0);
    }

    /// Names are slugged on the way in, so they have to be slugged on the way
    /// out too -- a server called `a b` writes `a_b_...log`, and a listing that
    /// matched the raw name would find none of its own files.
    #[test]
    fn a_name_that_was_slugged_still_finds_its_files() {
        let temp = Temp::new("list-slug");
        let store = temp.store();
        store
            .open(true, 9, "prod box", "chat-a")
            .response("anything");
        assert_eq!(store.list("prod box").len(), 1);
    }

    /// Newest first: the transcript worth opening is nearly always the last
    /// one, and a list that puts it at the bottom makes the common case the
    /// longest reach.
    #[test]
    fn the_listing_is_newest_first() {
        let temp = Temp::new("list-order");
        let store = temp.store();
        let first = store.open(true, 9, "host", "chat-a");
        first.response("anything");
        let second = store.open(true, 9, "host", "chat-b");
        second.response("anything");
        // Two files opened in the same second share a modification time, so the
        // order has to be settled by something else as well -- otherwise this
        // test passes or fails on where the second boundary happened to fall.
        let older = first.file().unwrap().to_path_buf();
        let long_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        filetime_set(&older, long_ago);

        let listed = store.list("host");
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed[0].path,
            second.file().unwrap().display().to_string(),
            "the newer file comes first"
        );
    }

    /// Sets a file modification time, so an ordering test does not depend on
    /// the clock ticking between two calls.
    fn filetime_set(path: &Path, when: std::time::SystemTime) {
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(when).unwrap();
    }

    /// The page hands back a path it was given, and this refuses to take that on
    /// trust.
    ///
    /// It is a command that deletes a file, reachable from a web page. The
    /// listing is the only thing that ever produces these paths today, which is
    /// exactly the argument that stops being true the first time something else
    /// calls it.
    #[test]
    fn only_a_file_inside_the_log_directory_can_be_removed() {
        let temp = Temp::new("remove");
        let store = temp.store();
        let session = store.open(true, 9, "host", "chat-a");
        session.response("anything");
        let mine = session.file().unwrap().display().to_string();

        let outside = std::env::temp_dir().join("tshell-not-a-log.log");
        std::fs::write(&outside, "x").unwrap();

        assert!(
            !store.remove(&outside.display().to_string()),
            "outside the directory"
        );
        assert!(outside.exists(), "and it is still there");

        // Climbing out and back in is the same refusal: the check is on where
        // the path resolves to, not on what it looks like.
        let climbed = temp
            .0
            .join("..")
            .join("tshell-not-a-log.log")
            .display()
            .to_string();
        assert!(!store.remove(&climbed));
        assert!(outside.exists());

        assert!(store.remove(&mine), "a file this store wrote");
        assert!(!std::path::Path::new(&mine).exists());

        let _ = std::fs::remove_file(&outside);
    }

    /// A path that is gone is not an error worth reporting: two panels showing
    /// the same list, one delete, and the second click has nothing to do.
    #[test]
    fn removing_what_is_already_gone_says_so_without_failing() {
        let temp = Temp::new("remove-twice");
        let store = temp.store();
        let session = store.open(true, 9, "host", "chat-a");
        session.response("anything");
        let mine = session.file().unwrap().display().to_string();
        assert!(store.remove(&mine));
        assert!(!store.remove(&mine));
    }

    /// Only transcripts. The directory holds nothing else today, and the day it
    /// does, this command must not be the thing that empties it.
    #[test]
    fn only_a_log_file_can_be_removed() {
        let temp = Temp::new("remove-kind");
        let store = temp.store();
        let other = temp.0.join("notes.txt");
        std::fs::write(&other, "x").unwrap();
        assert!(!store.remove(&other.display().to_string()));
        assert!(other.exists());
    }

    /// Opening a panel writes nothing.
    ///
    /// It used to write a file with a header the moment a panel opened, which
    /// meant a directory of empty transcripts for every time the assistant was
    /// opened and closed without being spoken to -- and, worse, it meant the
    /// pruning that keeps the last N transcripts was counting those. A file per
    /// glance pushed out the file per conversation.
    #[test]
    fn opening_a_panel_creates_no_file() {
        let temp = Temp::new("lazy-open");
        let store = temp.store();
        let session = store.open(true, 9, "host", "chat-a");
        assert!(session.active(), "logging is on");
        assert!(session.file().is_none(), "but nothing is written yet");
        assert_eq!(store.list("host").len(), 0);
    }

    /// The first thing actually sent is what creates it, header and all.
    #[test]
    fn the_first_request_creates_the_file() {
        let temp = Temp::new("lazy-first");
        let store = temp.store();
        let session = store.open(true, 9, "host", "chat-a");
        session.request(&LogRequest {
            endpoint: "https://example.invalid/v1/chat/completions",
            params: &json!({ "model": "m" }),
            system: "system prompt",
            messages: vec![("user".into(), "hello".into())],
        });
        assert!(session.file().is_some());
        assert_eq!(store.list("host").len(), 1);

        let text = read(&session);
        assert!(
            text.contains("# tshell AI transcript"),
            "the header is still first"
        );
        assert!(text.contains("hello"), "and the request follows it");
    }

    #[test]
    fn reopening_a_chat_reuses_its_existing_transcript() {
        let temp = Temp::new("resume");
        let store = temp.store();
        let first = store.open(true, 9, "host", "chat-a");
        first.request(&LogRequest {
            endpoint: "https://example.invalid/v1/chat/completions",
            params: &json!({ "model": "m" }),
            system: "system prompt",
            messages: vec![("user".into(), "first".into())],
        });
        let file = first.file().unwrap();

        let resumed = store.resume(true, 9, "host", "chat-a", 1);
        resumed.request(&LogRequest {
            endpoint: "https://example.invalid/v1/chat/completions",
            params: &json!({ "model": "m" }),
            system: "system prompt",
            messages: vec![
                ("user".into(), "first".into()),
                ("assistant".into(), "working".into()),
                ("user".into(), "continue".into()),
            ],
        });

        assert_eq!(resumed.file().as_deref(), Some(file.as_path()));
        assert_eq!(store.list("host").len(), 1);
        let text = read(&resumed);
        assert_eq!(text.matches("[user]\nfirst").count(), 1);
        assert_eq!(text.matches("[user]\ncontinue").count(), 1);
    }

    /// Pruning counts what has been written, and it happens when a file is
    /// actually created -- so a panel that is never spoken to cannot evict a
    /// transcript that was.
    #[test]
    fn a_panel_that_says_nothing_evicts_nothing() {
        let temp = Temp::new("lazy-prune");
        let store = temp.store();
        let mut real = Vec::new();
        for id in ["a", "b"] {
            let session = store.open(true, 2, "host", id);
            session.response("an answer");
            real.push(
                session
                    .file()
                    .expect("a transcript that was written to")
                    .to_path_buf(),
            );
        }
        assert_eq!(store.list("host").len(), 2);

        // Three panels opened and never used.
        for id in ["c", "d", "e"] {
            let _ = store.open(true, 2, "host", id);
        }

        // By identity, not by count: eager creation kept the count at two as
        // well, by replacing both transcripts with empty ones.
        let left: Vec<String> = store
            .list("host")
            .into_iter()
            .map(|file| file.path)
            .collect();
        for path in real {
            assert!(
                left.contains(&path.display().to_string()),
                "{path:?} survived"
            );
        }
    }

    fn read(session: &LogSession) -> String {
        std::fs::read_to_string(session.file().expect("a file, once written to")).unwrap()
    }

    #[test]
    fn logging_off_writes_nothing_at_all() {
        let temp = Temp::new("off");
        let session = temp.store().open(false, 20, "web-1", "chat-1");
        assert!(!session.active());
        assert!(session.file().is_none());
        session.response("would have been written");
        assert!(std::fs::read_dir(&temp.0).unwrap().next().is_none());
    }

    #[test]
    fn the_file_opens_with_a_header_that_says_what_it_is() {
        let temp = Temp::new("header");
        let session = temp.store().open(true, 20, "web-1", "chat-1");
        assert!(session.active());
        // The header goes in when the file does, which is at the first record.
        session.response("anything");
        let text = read(&session);
        assert!(text.starts_with("# tshell AI transcript -- web-1\n"));
        assert!(text.contains("NOT masked"));
    }

    #[test]
    fn only_what_is_new_in_the_conversation_is_written() {
        let temp = Temp::new("incremental");
        let session = temp.store().open(true, 20, "web-1", "chat-1");
        let params = json!({ "model": "m", "temperature": 0 });

        session.request(&LogRequest {
            endpoint: "http://x/chat/completions",
            params: &params,
            system: "SYSTEM ONE",
            messages: vec![("user".into(), "first".into())],
        });
        session.response("an answer");
        session.request(&LogRequest {
            endpoint: "http://x/chat/completions",
            params: &params,
            system: "SYSTEM ONE",
            messages: vec![
                ("user".into(), "first".into()),
                ("assistant".into(), "an answer".into()),
                ("user".into(), "second".into()),
            ],
        });

        let text = read(&session);
        assert_eq!(
            text.matches("SYSTEM ONE").count(),
            1,
            "the prompt is written once"
        );
        assert_eq!(
            text.matches("first").count(),
            1,
            "an old turn is not repeated"
        );
        assert_eq!(text.matches("second").count(), 1);
        assert_eq!(
            text.matches("an answer").count(),
            1,
            "the model's turn is the response record, not a request one"
        );
        assert!(text.contains("model:m"));
        assert!(text.contains("endpoint:http://x/chat/completions"));
    }

    /// Two panels opened within one second used to land on the same name, and
    /// the second truncated the first: one file holding two conversations, each
    /// with a `Progress` of its own. What that produces reads like a client that
    /// has gone wrong -- every panel rewrites the system prompt and the whole
    /// conversation the first time it speaks, because as far as its own progress
    /// is concerned nothing has been written yet.
    /// The name says which machine, which conversation, and when -- in that
    /// order, because the machine is what someone scanning the directory is
    /// looking for and the id is what they match against a chat still on screen.
    #[test]
    fn the_name_carries_the_machine_the_conversation_and_the_time() {
        let temp = Temp::new("naming");
        let session = temp.store().open(true, 10, "10.0.0.1", "a1b2c3d4");
        session.response("anything");
        let name = session
            .file()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();

        assert!(name.starts_with("10.0.0.1_a1b2c3d4_"), "name was {name}");
        assert!(name.ends_with(".log"), "name was {name}");
        // `<machine>_<id>_<date>_<time>.log`
        let stamp = name
            .trim_start_matches("10.0.0.1_a1b2c3d4_")
            .trim_end_matches(".log");
        let parts: Vec<&str> = stamp.split('_').collect();
        assert_eq!(parts.len(), 2, "date and time, in {name}");
        assert_eq!(parts[0].split('-').count(), 3, "a date, in {name}");
        assert_eq!(parts[1].split('-').count(), 3, "a time, in {name}");
    }

    /// One conversation, one transcript. A panel outlives its conversations, and
    /// putting them all in one file reads as a single exchange in which the model
    /// keeps forgetting everything and re-reading its prompt.
    #[test]
    fn each_conversation_gets_a_file_of_its_own() {
        let temp = Temp::new("perchat");
        let store = temp.store();
        let first = store.open(true, 10, "web-1", "chat-one");
        let second = store.open(true, 10, "web-1", "chat-two");

        first.response("BEFORE THE NEW CHAT");
        second.response("AFTER IT");

        let one = read(&first);
        let two = read(&second);
        assert!(one.contains("BEFORE THE NEW CHAT") && !one.contains("AFTER IT"));
        assert!(two.contains("AFTER IT") && !two.contains("BEFORE THE NEW CHAT"));
        assert!(first.file() != second.file());
    }

    #[test]
    fn two_sessions_opened_in_the_same_second_do_not_share_a_file() {
        let temp = Temp::new("collide");
        // The same conversation id on purpose: two ids would differ in the name
        // anyway, and what is being pinned here is the second-resolution stamp.
        let first = temp.store().open(true, 10, "10.0.0.1", "chat-1");
        let second = temp.store().open(true, 10, "10.0.0.1", "chat-1");

        // Written to first: the name is claimed when the file is made, and the
        // file is made by the first record. Two panels that both stayed silent
        // never collide because neither ever has a name.
        first.response("FIRST PANEL");
        second.response("SECOND PANEL");

        let one = first.file().expect("the first has a file");
        let two = second.file().expect("the second has a file");
        assert_ne!(one, two, "both panels wrote to {}", one.display());

        let text_one = read(&first);
        let text_two = read(&second);
        assert!(text_one.contains("FIRST PANEL") && !text_one.contains("SECOND PANEL"));
        assert!(text_two.contains("SECOND PANEL") && !text_two.contains("FIRST PANEL"));
        // One header apiece is what says each file is whole rather than restarted.
        assert_eq!(text_one.matches("# tshell AI transcript").count(), 1);
        assert_eq!(text_two.matches("# tshell AI transcript").count(), 1);
    }

    #[test]
    fn a_changed_system_prompt_is_written_again_whole() {
        let temp = Temp::new("system");
        let session = temp.store().open(true, 20, "web-1", "chat-1");
        let params = json!({});
        for system in ["ONE", "ONE", "TWO"] {
            session.request(&LogRequest {
                endpoint: "e",
                params: &params,
                system,
                messages: vec![("user".into(), "x".into())],
            });
        }
        let text = read(&session);
        assert_eq!(text.matches("[system]\nONE").count(), 1);
        assert_eq!(text.matches("[system]\nTWO").count(), 1);
    }

    #[test]
    fn a_new_conversation_starts_the_count_over() {
        let temp = Temp::new("newchat");
        let session = temp.store().open(true, 20, "web-1", "chat-1");
        let params = json!({});
        session.request(&LogRequest {
            endpoint: "e",
            params: &params,
            system: "s",
            messages: vec![("user".into(), "a".into()), ("user".into(), "b".into())],
        });
        session.request(&LogRequest {
            endpoint: "e",
            params: &params,
            system: "s",
            messages: vec![("user".into(), "fresh".into())],
        });
        assert!(read(&session).contains("fresh"));
    }

    #[test]
    fn records_are_written_verbatim_with_their_newlines() {
        let temp = Temp::new("verbatim");
        let session = temp.store().open(true, 20, "web-1", "chat-1");
        session.response("line one\nline two");
        let text = read(&session);
        assert!(text.contains("content:line one\nline two\n\n"));
        session.reasoning("thought");
        assert!(read(&session).contains("type:reasoning content:thought"));
    }

    /// Oldest means oldest by the clock, and the times are set rather than
    /// assumed: two files written in the same millisecond would otherwise settle
    /// it by whatever order the directory happened to be read in.
    ///
    /// The names here are deliberately in the opposite order to the ages. Sorting
    /// by name -- which is what this did while the stamp came first -- would keep
    /// the wrong one, and would do it silently.
    fn write_aged(dir: &Path, name: &str, hours_ago: u64) {
        let path = dir.join(name);
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"old").unwrap();
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(hours_ago * 3600);
        file.set_modified(when).unwrap();
    }

    #[test]
    fn the_oldest_files_go_when_the_count_is_reached() {
        let temp = Temp::new("prune");
        let store = temp.store();
        write_aged(&temp.0, "zzz_old.log", 48);
        write_aged(&temp.0, "aaa_recent.log", 1);

        let session = store.open(true, 2, "web-1", "chat-1");
        assert!(session.active());
        // Pruning runs when the file is made, which is now the first record.
        session.response("anything");

        let names: Vec<String> = std::fs::read_dir(&temp.0)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().into_string().unwrap())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(!names.contains(&"zzz_old.log".to_string()), "{names:?}");
        assert!(names.contains(&"aaa_recent.log".to_string()), "{names:?}");
    }

    #[test]
    fn a_server_name_becomes_a_filename_without_losing_its_language() {
        assert_eq!(slug("交易机"), "交易机");
        assert_eq!(slug("web 1/prod"), "web_1_prod");
        assert_eq!(slug("..hidden"), "hidden");
        assert_eq!(slug("///"), "server");
        assert_eq!(slug("").len(), "server".len());
        assert_eq!(slug(&"x".repeat(80)).chars().count(), 40);
    }

    #[test]
    fn a_parameter_reads_the_way_it_would_in_the_body() {
        assert_eq!(plain(&json!(0)), "0");
        assert_eq!(plain(&json!(true)), "true");
        assert_eq!(plain(&json!("high")), "high");
        assert_eq!(
            plain(&json!({ "type": "disabled" })),
            "{\"type\":\"disabled\"}"
        );
    }

    #[test]
    fn the_two_stamps_have_the_shape_the_reader_expects() {
        let at = stamp_ms();
        assert_eq!(at.len(), "20260709-12:00:00.212".len(), "{at}");
        assert_eq!(&at[8..9], "-");
        let file = stamp_file();
        assert_eq!(file.len(), "2026-07-09_12-00-00".len(), "{file}");
    }
}
