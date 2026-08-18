//! What the model may ask for, and what the panel is told happened.
//!
//! Both halves of the protocol live here rather than beside the loop that uses
//! them, because the stores, the file layer and the page bridge all speak in
//! these and none of them should have to depend on the loop to do it.

use serde::{Deserialize, Serialize};

use super::policy::command::Verdict;
use super::policy::risk::RiskReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileOpKind {
    Write,
    Append,
    Edit,
}

impl FileOpKind {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Append => "append",
            Self::Edit => "edit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryScope {
    Global,
    Server,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferKind {
    Upload,
    Download,
}

/// Text encodings a remote file is read and written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileEncoding {
    Utf8,
    Gb18030,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ActionKind {
    Run,
    Skill,
    Write,
    Append,
    Edit,
    Remember,
    Upload,
    Download,
}

impl ActionKind {
    /// The verb as the model spells it, which is also the i18n-free wire form.
    pub fn tag(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::Skill => "skill",
            Self::Write => "write",
            Self::Append => "append",
            Self::Edit => "edit",
            Self::Remember => "remember",
            Self::Upload => "upload",
            Self::Download => "download",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "run" => Self::Run,
            "skill" => Self::Skill,
            "write" => Self::Write,
            "append" => Self::Append,
            "edit" => Self::Edit,
            "remember" => Self::Remember,
            "upload" => Self::Upload,
            "download" => Self::Download,
            _ => return None,
        })
    }

    pub fn as_file_op(self) -> Option<FileOpKind> {
        Some(match self {
            Self::Write => FileOpKind::Write,
            Self::Append => FileOpKind::Append,
            Self::Edit => FileOpKind::Edit,
            _ => return None,
        })
    }

    pub fn as_transfer(self) -> Option<TransferKind> {
        Some(match self {
            Self::Upload => TransferKind::Upload,
            Self::Download => TransferKind::Download,
            _ => return None,
        })
    }
}

/// One thing the model asked for.
///
/// Every field is optional because one object carries all eight verbs, and the
/// loop reads only the ones its verb uses. Keeping them in one struct rather
/// than an enum is deliberate: the parser has to accept whatever arrives and
/// decide afterwards, and a malformed action with the wrong keys must still be
/// readable enough to say so.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentAction {
    pub action: Option<ActionKind>,
    pub command: Option<String>,
    pub why: Option<String>,
    /// The memory action's fact.
    pub text: Option<String>,
    /// The file actions. Content is carried verbatim, newlines and all.
    pub path: Option<String>,
    pub content: Option<String>,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    /// The memory actions. Anything unrecognised narrows to the current server.
    pub scope: Option<MemoryScope>,
    /// The transfer actions: the sources, and the directory they land in.
    pub paths: Vec<String>,
    pub to: Option<String>,
    /// The skill action: which skill, and which file inside it.
    pub name: Option<String>,
    pub file: Option<String>,
}

/// Why a step that would have been confirmed went ahead without being.
///
/// Carried on the event so the thread can mark it. Without this a transcript
/// read back a week later shows a row of changes indistinguishable from ones the
/// user approved one by one, which is a quiet lie about what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Unconfirmed {
    Auto,
    Trusted,
}

/// How a memory write turned out.
///
/// `undone` is the panel's, not the store's: it is what a remembered card
/// becomes after the user presses undo, so a reloaded thread reports what
/// actually stands rather than replaying the moment before it was taken back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MemoryOutcome {
    Ok,
    Duplicate,
    Full,
    Failed,
    Off,
    Undone,
    /// Refused before the store saw it, for being too long to be one fact.
    ///
    /// Its own outcome rather than `failed`, because the two read completely
    /// differently to whoever is looking at the card: `failed` says the write
    /// went wrong and invites them to check the disk, and this says the line was
    /// never a fact worth writing.
    Oversize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SkillOutcome {
    Ok,
    Unknown,
    Disabled,
    Missing,
    Unreadable,
    Off,
    Repeat,
}

/// How much the assistant asks before it acts.
///
/// The three are strictly ordered -- each asks for less than the one before it
/// -- and none of them touches what is refused outright. A destructive command
/// and a write to /dev are blocked in `auto` exactly as they are in `ask`:
/// confirmation is for what is risky, refusal is for what cannot be taken back,
/// and turning the asking off is not a statement about the second one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentMode {
    /// Read-only commands run; everything else is confirmed. Trusted dirs inert.
    Ask,
    /// As `ask`, except file actions inside a trusted directory go through.
    Trust,
    /// Nothing is confirmed. Refusals still refuse.
    Auto,
}

/// What one command did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandResult {
    pub output: String,
    pub exit_code: i32,
    pub timed_out: bool,
    pub truncated: bool,
}

/// Which half of a transfer is being reported.
///
/// Scanning knows how much it has found so far but not how much there is, so the
/// page draws a total it is building rather than a percentage it cannot know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TransferPhase {
    Scanning,
    Transferring,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOverall {
    pub done_files: u32,
    pub total_files: u32,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferCurrent {
    pub name: String,
    pub transferred: u64,
    pub total: u64,
}

/// How far a transfer has got.
///
/// # Why this is a type and not a `json!`
///
/// It was a `json!` built by hand in `bridge.rs`, and it was wrong in every
/// field: it read `done` and `filesDone` off an engine that sends `doneBytes` and
/// `doneFiles`, and it sent them flat to a page that reads `overall.doneBytes`.
/// Three shapes, none of them the same, and nothing to notice it -- the payload
/// was all nulls, `paintProgress` threw on the first event, and the bar sat at
/// zero while the file transferred perfectly.
///
/// The nesting is the page's, not a preference: `chat.js` and `transfer.js` both
/// read `phase`, `overall` and `current`, and both pages are the extension's,
/// unedited. Expressing that as a type is what makes serde responsible for the
/// names instead of whoever last touched the call site.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferProgress {
    pub phase: TransferPhase,
    pub overall: TransferOverall,
    pub current: TransferCurrent,
}

/// What the panel is told, as it happens.
///
/// Serialised tagged by `type` with camelCase fields, because this is handed to
/// `chat.js` unchanged -- that page is the extension's, unedited, and its reader
/// is written against exactly these names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AgentEvent {
    /// `step` counts from 1 within the task, so the panel can say how far along it is.
    Thinking { step: u32 },
    #[serde(rename_all = "camelCase")]
    Command {
        command: String,
        why: String,
        verdict: Verdict,
        #[serde(skip_serializing_if = "Option::is_none")]
        blocker: Option<String>,
        /// Set when the verdict was `confirm` but the mode let it run unasked.
        #[serde(skip_serializing_if = "Option::is_none")]
        unconfirmed: Option<Unconfirmed>,
    },
    #[serde(rename_all = "camelCase")]
    Result { exit_code: i32, output: String, timed_out: bool, truncated: bool },
    /// The change a file action is about to make, drawn in the thread as well as
    /// the dialog.
    #[serde(rename_all = "camelCase")]
    File {
        kind: FileOpKind,
        path: String,
        exists: bool,
        size: String,
        preview: String,
        before: String,
        after: String,
        /// Shown only when it is not utf8, which is the case that needs saying.
        encoding: FileEncoding,
        /// The file line the shown text starts on. Absent leaves the gutter off.
        #[serde(skip_serializing_if = "Option::is_none")]
        line: Option<u32>,
    },
    /// A line written to or removed from memory. Reported rather than asked
    /// about: this is local text, not a change to the machine, and the card
    /// carries an undo so being told after the fact costs the user nothing.
    #[serde(rename_all = "camelCase")]
    Memory {
        scope: MemoryScope,
        text: String,
        outcome: MemoryOutcome,
        /// Present only on a line that was actually written, and only while live.
        #[serde(skip_serializing_if = "Option::is_none")]
        token: Option<String>,
    },
    /// A transfer that is about to start, drawn in the card the step opened.
    #[serde(rename_all = "camelCase")]
    Transfer { kind: TransferKind, sources: Vec<String>, target: String },
    /// Ticks while bytes move, several times a second. Deliberately not recorded
    /// with the rest of the thread: a finished transfer is described by its
    /// summary, and a replayed conversation showing a progress bar frozen at 43%
    /// would be reporting a moment rather than an outcome.
    #[serde(rename_all = "camelCase")]
    TransferProgress { progress: TransferProgress },
    /// A skill the model pulled into the conversation. Reported rather than
    /// asked about, like a memory write and for the same reason.
    #[serde(rename_all = "camelCase")]
    Skill {
        id: String,
        /// Which file inside it, relative to the skill's own directory.
        file: String,
        outcome: SkillOutcome,
        /// Present when the file ran past `maxChars` and lost its middle.
        #[serde(skip_serializing_if = "Option::is_none")]
        truncated: Option<bool>,
    },
    /// A directory the user has just stopped being asked about.
    ///
    /// Reported the way a remembered line is, and for the same reason: it is a
    /// lasting change to how the assistant behaves, made in one keystroke, and
    /// the dialog that made it is gone a moment later.
    Trusted { dir: String },
    Refused { command: String, reasons: Vec<String> },
    Declined { command: String },
    /// A fragment of an answer, as it arrives. Transient like `thinking`: the
    /// whole text follows in `reply` or `summary`, which is what a replayed
    /// thread draws.
    Delta { text: String },
    /// The model's own thinking, as it arrives. Transient like `delta`, and for
    /// a stronger reason: it is the longest thing in a step and the least worth
    /// keeping, being the road to the answer rather than the answer.
    Reasoning { text: String },
    /// What one request cost. `estimated` marks the counts as this client's own
    /// arithmetic rather than the endpoint's.
    #[serde(rename_all = "camelCase")]
    Usage {
        prompt: u32,
        completion: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        estimated: Option<bool>,
    },
    /// How the answer arrived, so "is it streaming?" has an answer on screen.
    #[serde(rename_all = "camelCase")]
    Transport { streamed: bool, frames: u32, first_ms: u64, total_ms: u64 },
    /// How much conversation is being carried, after any folding.
    Context { chars: u32 },
    /// A failed request about to be made again, so the pause has a reason on screen.
    Retry { attempt: u32, kind: String },
    /// The endpoint refused one of the optional request fields, so it was sent
    /// again without it.
    Degraded { fields: Vec<String> },
    Reply { text: String },
    /// `message` is English and is what a consumer without a string table shows.
    /// `code` names the failure so the page can show it in the user's language,
    /// and so a stored transcript still reads correctly after the language changes.
    Error {
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    StepLimit { steps: u32 },
    Stopped,
    Idle,
}

/// The i18n key for one refusal reason, in the `risk<Capitalised>` spelling the
/// string table uses.
pub fn risk_key(reason: RiskReason) -> String {
    let key = reason.key();
    let mut head = key.chars();
    match head.next() {
        Some(first) => format!("risk{}{}", first.to_uppercase(), head.as_str()),
        None => "risk".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact keys `paintProgress` reads, spelled out.
    ///
    /// This is the test that was missing. The progress payload was built by hand
    /// and every field name in it was wrong -- flat where the page nests, `done`
    /// where the engine says `doneBytes` -- and nothing anywhere compared the two
    /// spellings. The transfer worked; the bar sat at zero; there was no error to
    /// find, because the page threw on `undefined.doneFiles` and the throw went
    /// nowhere.
    ///
    /// If a field here is renamed, this fails. That is the entire point: both
    /// pages are the extension's, unedited, so the names are theirs and not ours.
    #[test]
    fn transfer_progress_serialises_the_way_both_pages_read_it() {
        let event = AgentEvent::TransferProgress {
            progress: TransferProgress {
                phase: TransferPhase::Transferring,
                overall: TransferOverall {
                    done_files: 2,
                    total_files: 5,
                    done_bytes: 1024,
                    total_bytes: 4096,
                },
                current: TransferCurrent {
                    name: "big.log".into(),
                    transferred: 512,
                    total: 2048,
                },
            },
        };
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "transferProgress");
        // `chat.js`: const overall = progress.overall; const current = progress.current;
        assert_eq!(json["progress"]["phase"], "transferring");
        assert_eq!(json["progress"]["overall"]["doneFiles"], 2);
        assert_eq!(json["progress"]["overall"]["totalFiles"], 5);
        assert_eq!(json["progress"]["overall"]["doneBytes"], 1024);
        assert_eq!(json["progress"]["overall"]["totalBytes"], 4096);
        assert_eq!(json["progress"]["current"]["name"], "big.log");
        assert_eq!(json["progress"]["current"]["transferred"], 512);
        assert_eq!(json["progress"]["current"]["total"], 2048);
    }

    /// Scanning is the other half, and the page tells it apart by this one word.
    #[test]
    fn a_scanning_report_says_so() {
        let event = AgentEvent::TransferProgress {
            progress: TransferProgress {
                phase: TransferPhase::Scanning,
                overall: TransferOverall { total_files: 9, total_bytes: 99, ..Default::default() },
                current: TransferCurrent::default(),
            },
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["progress"]["phase"], "scanning");
        assert_eq!(json["progress"]["overall"]["totalFiles"], 9);
    }

    #[test]
    fn events_serialise_the_way_the_page_reads_them() {
        let event = AgentEvent::Result {
            exit_code: 1,
            output: "x".into(),
            timed_out: false,
            truncated: true,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "result");
        assert_eq!(json["exitCode"], 1);
        assert_eq!(json["timedOut"], false);
        assert_eq!(json["truncated"], true);
    }

    #[test]
    fn a_unit_event_still_carries_its_tag() {
        let json = serde_json::to_value(AgentEvent::Stopped).unwrap();
        assert_eq!(json["type"], "stopped");
        let json = serde_json::to_value(AgentEvent::StepLimit { steps: 12 }).unwrap();
        assert_eq!(json["type"], "stepLimit");
        assert_eq!(json["steps"], 12);
    }

    #[test]
    fn absent_options_are_left_out_rather_than_sent_as_null() {
        let event = AgentEvent::Command {
            command: "ls".into(),
            why: "look".into(),
            verdict: Verdict::Auto,
            blocker: None,
            unconfirmed: None,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["verdict"], "auto");
        assert!(json.get("blocker").is_none());
        assert!(json.get("unconfirmed").is_none());
    }

    #[test]
    fn risk_reasons_become_the_keys_the_table_is_written_in() {
        assert_eq!(risk_key(RiskReason::RemovesRoot), "riskRemovesRoot");
        assert_eq!(risk_key(RiskReason::WritesKernelInterface), "riskWritesKernelInterface");
    }
}
