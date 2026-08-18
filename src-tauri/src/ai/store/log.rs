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

/// Where one panel's exchange is written, or nowhere when logging is off.
pub struct LogSession {
    file: Option<PathBuf>,
    progress: Mutex<Progress>,
}

impl LogSession {
    /// The session handed out when logging is off. Costs a branch per call.
    pub fn inactive() -> Self {
        Self { file: None, progress: Mutex::new(Progress::default()) }
    }

    pub fn active(&self) -> bool {
        self.file.is_some()
    }

    /// Where it is being written, for the message that points the user at it.
    pub fn file(&self) -> Option<&Path> {
        self.file.as_deref()
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
        let Some(file) = &self.file else { return };
        let spaced = if head.is_empty() { String::new() } else { format!(" {head}") };
        let record = format!("time:{} type:{kind}{spaced} content:{content}\n\n", stamp_ms());
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
    chrono::Local::now().format("%Y%m%d-%H:%M:%S%.3f").to_string()
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
    let trimmed: String =
        mapped.trim_start_matches(['.', '_']).chars().take(40).collect();
    let trimmed = trimmed.trim_end_matches('.');
    if trimmed.is_empty() {
        "server".to_string()
    } else {
        trimmed.to_string()
    }
}

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
    pub fn open(
        &self,
        enabled: bool,
        keep: usize,
        server_name: &str,
        chat_id: &str,
    ) -> LogSession {
        if !enabled {
            return LogSession::inactive();
        }
        if std::fs::create_dir_all(&self.dir).is_err() {
            return LogSession::inactive();
        }
        // Before the file is created, so the count is of what is already there
        // and this session is never a candidate for its own pruning.
        self.prune(keep.saturating_sub(1));

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
        match self.create(&base, &header) {
            Some(file) => LogSession { file: Some(file), progress: Mutex::new(Progress::default()) },
            None => LogSession::inactive(),
        }
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
            match std::fs::OpenOptions::new().create_new(true).write(true).open(&path) {
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
    fn prune(&self, keep: usize) {
        let Ok(entries) = std::fs::read_dir(&self.dir) else { return };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "log"))
            .collect();
        // A file whose time cannot be read sorts oldest, so it is the first to go
        // rather than something that survives every sweep by being unreadable.
        names.sort_by_key(|path| {
            std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
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
            let dir = std::env::temp_dir().join(format!("tshell-log-{name}-{}", std::process::id()));
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

    fn read(session: &LogSession) -> String {
        std::fs::read_to_string(session.file().unwrap()).unwrap()
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
        assert_eq!(text.matches("SYSTEM ONE").count(), 1, "the prompt is written once");
        assert_eq!(text.matches("first").count(), 1, "an old turn is not repeated");
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

        let one = first.file().expect("the first has a file").to_path_buf();
        let two = second.file().expect("the second has a file").to_path_buf();
        assert_ne!(one, two, "both panels wrote to {}", one.display());

        first.response("FIRST PANEL");
        second.response("SECOND PANEL");

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
        let when =
            std::time::SystemTime::now() - std::time::Duration::from_secs(hours_ago * 3600);
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
        assert_eq!(plain(&json!({ "type": "disabled" })), "{\"type\":\"disabled\"}");
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
