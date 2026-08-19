//! Writing files from the assistant: what will change, and how the bytes get
//! there.
//!
//! # Two ways for the bytes to travel, and one way for the change to land
//!
//! Where the host has a channel of its own -- see `link.rs` -- the payload is one
//! SFTP write to a temporary file beside the target. Where it has not, the
//! payload goes the way it always did: through the shared terminal, as base64,
//! split into chunks that each fit on a line. Both are real paths. The second is
//! what a test takes and what any host without a second connection takes, so it
//! is kept working rather than kept as a rescue.
//!
//! Base64 is what the typed path uses because a shell command is one line here --
//! that is what the marker protocol in `shell.rs` is built on -- and file content
//! is the one thing that is not. A heredoc is the obvious answer and the wrong
//! one: its body would have to survive being typed into an interactive shell,
//! where a tab is completion, a newline is a PS2 continuation, and the terminal's
//! own encoding decides what the bytes mean. Base64 is one line by construction,
//! immune to quoting, and pure ASCII -- the last part matters because a gb18030
//! session would otherwise mangle UTF-8 on the way in. Chunks are a multiple of
//! four characters because base64 is only self-contained on that boundary.
//!
//! However the bytes arrived, the change lands the same way: the temporary is
//! moved into place by a shell command at the end. The move is atomic, so the
//! target is either the old file or the new one and never half of either; an
//! interrupted write leaves a stray temporary behind rather than a truncated
//! config file. Keeping that step in the shell is deliberate -- SFTP cannot
//! promise it, and it is also the line the user sees in their terminal saying
//! what was written.
//!
//! `edit` is a read, a match, and a write, with the match done here rather than
//! by `sed` on the far side. That is what makes "not found" and "found three
//! times" answerable at all, and it is what lets the confirmation dialog show a
//! diff the user can trust: the old text in it was read off the machine, not
//! recited by the model.
//!
//! The model deals in text and never in bytes -- what arrives from it is a string
//! of code points, so which bytes reach the disk is this module's decision alone.
//! An existing file keeps the encoding it already had, because it has readers
//! that were not part of the conversation; a new one is written UTF-8. Getting
//! this wrong is expensive and quiet: a GBK config rewritten as UTF-8 still
//! parses on the box that only reads ASCII keys, and turns to mojibake for
//! everyone else.

use std::future::Future;
use std::pin::Pin;

use base64::Engine;
use sha2::{Digest, Sha256};

use super::shell::{quote, RunOptions};
use super::types::{CommandResult, FileEncoding, FileOpKind};

/// Large enough for any configuration file, small enough to stay a paste, not a
/// transfer.
pub const MAX_FILE_BYTES: usize = 256 * 1024;
/// Base64 characters per command. A multiple of 4, and well inside a terminal line.
const CHUNK_CHARS: usize = 3072;

/*
 * Two ways to answer the same question, in one round trip.
 *
 * Where iconv exists -- which is nearly everywhere, it comes with glibc -- it
 * reads the whole file and answers exactly. Where it does not, a prefix comes
 * back instead and the answer is drawn from that: a sample can only be wrong
 * about a file whose first 64 KB are ASCII and whose non-ASCII is further in.
 */
const PROBE_PREFIX_BYTES: usize = 64 * 1024;
const UTF8_MARKER: &str = "tshell-utf8";
const OTHER_MARKER: &str = "tshell-other";

/// What the model asked for, straight out of the protocol.
#[derive(Debug, Clone)]
pub struct FileRequest {
    pub kind: FileOpKind,
    pub path: String,
    /// write, append: the text to put there.
    pub content: Option<String>,
    /// edit: the fragment to replace, which must occur exactly once.
    pub old_text: Option<String>,
    pub new_text: Option<String>,
}

/// A request that has been checked against the machine and is ready to confirm.
#[derive(Debug, Clone)]
pub struct FilePlan {
    pub kind: FileOpKind,
    pub path: String,
    pub exists: bool,
    /// Bytes carried: the whole file for write and edit, the addition for append.
    pub bytes: usize,
    /// What the dialog shows: the file as it will be, or the text being added.
    pub preview: String,
    /// edit only: the fragment being replaced and what replaces it.
    pub before: Option<String>,
    pub after: Option<String>,
    /// The file line the shown text starts on, counting from 1.
    ///
    /// 1 for a write, since that shows the file whole. For an edit it is where
    /// the fragment actually sits, which is the only number worth drawing: a
    /// fragment numbered from 1 would be a line number for a file nobody has.
    /// Both halves of the diff start there -- everything above the match is
    /// untouched, so the old text and the new text begin on the same line.
    pub line: Option<u32>,
    /// What is actually transferred. Not shown; `preview` is what the user reads.
    pub payload: String,
    /// The bytes `payload` becomes: the file's own encoding, or utf8 for a new file.
    pub encoding: FileEncoding,
}

/// A failure the model can do something about: a path that is a directory, an
/// edit that matched nothing or matched twice. Carried back to it as an
/// observation, not raised as an error, because the task is not over -- the next
/// attempt is narrower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOpError(pub String);

impl std::fmt::Display for FileOpError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(&self.0)
    }
}

fn fail<T>(message: impl Into<String>) -> Result<T, FileOpError> {
    Err(FileOpError(message.into()))
}

/// What a path turned out to be, when the byte channel could answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Directory,
    File,
    Absent,
}

/// How a plan reaches the machine.
///
/// `run` is the shell, and it is the only method that has to be there: it is what
/// a test answers from a table instead of a server, and it is what carries every
/// step the user is meant to watch.
///
/// The other three are the assistant's own connection -- see `link.rs` -- and
/// they are optional in the strong sense. `None` does not mean "it failed", it
/// means "there is no such channel here", and every one of them has a shell
/// answer to fall back to. That is what keeps the fallback honest: it is not a
/// path that only runs when something breaks, it is the path a test takes, and
/// the path any host without a second connection takes.
pub trait FileRunner: Send + Sync {
    fn run<'a>(
        &'a self,
        command: String,
        options: RunOptions,
    ) -> Pin<Box<dyn Future<Output = CommandResult> + Send + 'a>>;

    /// A file's bytes, exactly. `None` when there is no channel to ask.
    fn fetch<'a>(
        &'a self,
        _path: String,
    ) -> Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, String>>> + Send + 'a>> {
        Box::pin(async { None })
    }

    /// Puts bytes at a path, creating or truncating it.
    ///
    /// Only ever called with a temporary path. What makes the change visible in
    /// the target is a shell command afterwards, because that is the step that
    /// has to be atomic and the step the user has to see.
    fn stash<'a>(
        &'a self,
        _path: String,
        _bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Option<Result<(), String>>> + Send + 'a>> {
        Box::pin(async { None })
    }

    /// What is at a path. `None` when there is no channel, or when it could not
    /// tell -- both mean "ask the shell instead".
    fn probe<'a>(
        &'a self,
        _path: String,
    ) -> Pin<Box<dyn Future<Output = Option<Presence>> + Send + 'a>> {
        Box::pin(async { None })
    }
}

pub fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        None => String::new(),
        Some(0) => "/".to_string(),
        Some(at) => path[..at].to_string(),
    }
}

/// Next to the target, so the final move stays on one filesystem and is atomic.
pub fn temp_path_for(path: &str, nonce: &str) -> String {
    let dir = dir_of(path);
    let name = format!(".tshell-{nonce}.tmp");
    match dir.as_str() {
        "/" => format!("/{name}"),
        "" => name,
        dir => format!("{dir}/{name}"),
    }
}

pub fn base64_chunks(payload: &[u8], size: usize) -> Vec<String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(payload);
    encoded
        .as_bytes()
        .chunks(size)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect()
}

/// The bytes a payload becomes. The only place text turns into a file's contents.
pub fn encode_content(payload: &str, encoding: FileEncoding) -> Vec<u8> {
    match encoding {
        FileEncoding::Utf8 => payload.as_bytes().to_vec(),
        FileEncoding::Gb18030 => encoding_rs::GB18030.encode(payload).0.into_owned(),
    }
}

/// Valid UTF-8 is exact; anything else is read as gb18030, which is a superset of
/// GBK and GB2312.
pub fn decode_content(data: &[u8], encoding: FileEncoding) -> (String, bool) {
    match encoding {
        FileEncoding::Utf8 => match std::str::from_utf8(data) {
            Ok(text) => (text.to_string(), false),
            Err(_) => (String::from_utf8_lossy(data).into_owned(), true),
        },
        FileEncoding::Gb18030 => {
            let (text, _, had_errors) = encoding_rs::GB18030.decode(data);
            (text.into_owned(), had_errors)
        }
    }
}

pub fn is_utf8(data: &[u8]) -> bool {
    std::str::from_utf8(data).is_ok()
}

/// Trims a multi-byte character the prefix was cut in half of, so a sample of a
/// UTF-8 file is not read as some other encoding purely because it stopped
/// between two bytes of one character.
pub fn without_split_character(data: &[u8]) -> &[u8] {
    for back in 1..=3usize.min(data.len()) {
        let lead = data[data.len() - back];
        if lead < 0xc0 {
            continue;
        }
        let width = if lead >= 0xf0 {
            4
        } else if lead >= 0xe0 {
            3
        } else {
            2
        };
        return if width > back { &data[..data.len() - back] } else { data };
    }
    data
}

/// What a file already is. UTF-8 covers ASCII, so a plain config file answers
/// utf8 and a new file never gets here at all -- gb18030 is only ever the answer
/// for bytes that cannot be anything else.
pub fn detect_encoding(data: &[u8], partial: bool) -> FileEncoding {
    let sample = if partial { without_split_character(data) } else { data };
    if is_utf8(sample) {
        FileEncoding::Utf8
    } else {
        FileEncoding::Gb18030
    }
}

/// A NUL byte is what tells a program from a document, and is what git uses too.
fn looks_binary(data: &[u8]) -> bool {
    data.iter().take(8000).any(|byte| *byte == 0)
}

pub fn build_encoding_probe(path: &str) -> String {
    let file = quote(path);
    format!(
        "if command -v iconv >/dev/null 2>&1; then \
         if iconv -f UTF-8 -t UTF-8 {file} >/dev/null 2>&1; then echo {UTF8_MARKER}; \
         else echo {OTHER_MARKER}; fi; \
         else head -c {PROBE_PREFIX_BYTES} {file} | base64; fi"
    )
}

pub fn read_encoding_probe(output: &str) -> FileEncoding {
    let text = output.trim();
    if text.ends_with(UTF8_MARKER) {
        return FileEncoding::Utf8;
    }
    if text.ends_with(OTHER_MARKER) {
        return FileEncoding::Gb18030;
    }
    let packed: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let data = base64::engine::general_purpose::STANDARD.decode(&packed).unwrap_or_default();
    detect_encoding(&data, true)
}

pub fn build_probe(path: &str) -> String {
    let target = quote(path);
    format!("if [ -d {target} ]; then echo dir; elif [ -e {target} ]; then echo file; else echo none; fi")
}

pub fn build_read(path: &str) -> String {
    format!("base64 {}", quote(path))
}

/// `>` on the first chunk creates or truncates; everything after it appends.
pub fn build_chunk_commands(payload: &[u8], target: &str, append: bool) -> Vec<String> {
    let file = quote(target);
    let chunks = base64_chunks(payload, CHUNK_CHARS);
    if chunks.is_empty() {
        return if append { Vec::new() } else { vec![format!(": > {file}")] };
    }
    chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            let arrow = if index == 0 && !append { ">" } else { ">>" };
            format!("printf %s {} | base64 -d {arrow} {file}", quote(chunk))
        })
        .collect()
}

/// The permission bits are copied from the file being replaced, because a move
/// brings the temporary file's own mode with it and a 0600 sshd_config is a
/// broken one. Ownership needs root and is left alone; when the mover is not the
/// owner the file changes hands, which is the honest cost of replacing rather
/// than rewriting.
pub fn build_commit(path: &str, temp: &str, exists: bool) -> String {
    let file = quote(path);
    let staged = quote(temp);
    let keep_mode =
        if exists { format!("chmod --reference={file} {staged} 2>/dev/null; ") } else { String::new() };
    format!("{keep_mode}mv -f -- {staged} {file}")
}

/// The commit for an append, when the addition was staged rather than typed.
///
/// `cat` rather than a second `mv`, because an append has a file to land at the
/// end of and a move would replace it. `rm` runs whatever `cat` did, so a failed
/// append does not also leave the staged bytes lying beside the target -- and it
/// runs after, so a `cat` that failed on a full disk still reports its own status
/// as the command's.
pub fn build_append_commit(path: &str, temp: &str) -> String {
    let file = quote(path);
    let staged = quote(temp);
    format!("cat -- {staged} >> {file}; status=$?; rm -f -- {staged}; exit $status")
}

/// Size and digest in one step, digest optional: not every box has sha256sum.
pub fn build_verify(path: &str) -> String {
    let file = quote(path);
    format!("wc -c < {file} | tr -d ' \\n'; printf ' '; sha256sum {file} 2>/dev/null | cut -d' ' -f1")
}

pub fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

/// A text file ends in a newline. Models routinely forget, and tools notice.
fn end_with_newline(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

fn failed(result: &CommandResult) -> bool {
    result.timed_out || result.exit_code != 0
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn format_bytes(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    }
}

fn silent() -> RunOptions {
    RunOptions { silent: true, ..Default::default() }
}

fn silent_raw() -> RunOptions {
    RunOptions { silent: true, raw_output: true, ..Default::default() }
}

/// Checks the request against the machine and works out exactly what will land.
///
/// Everything here is read-only and silent: the user has not agreed to anything
/// yet, so nothing may change and nothing needs to appear in their terminal.
pub async fn plan_file_op(
    request: &FileRequest,
    run: &dyn FileRunner,
) -> Result<FilePlan, FileOpError> {
    let path = request.path.trim().to_string();
    if path.is_empty() {
        return fail("The path is empty. Give an absolute path to the file.");
    }
    if path.ends_with('/') {
        return fail(format!("{path} names a directory. Give the path of a file."));
    }

    // One `stat` where there is a channel to ask on; the shell's `[ -d ]` where
    // there is not. Both answer the same three things, and the shell's answer is
    // the one that has to cope with a machine that said something unexpected.
    let state = match run.probe(path.clone()).await {
        Some(Presence::Directory) => "dir".to_string(),
        Some(Presence::File) => "file".to_string(),
        Some(Presence::Absent) => "none".to_string(),
        None => {
            let probe = run.run(build_probe(&path), silent()).await;
            if probe.timed_out {
                return fail(format!("Checking {path} was interrupted before it answered."));
            }
            let state =
                probe.output.trim().lines().next_back().unwrap_or_default().trim().to_string();
            if state != "dir" && state != "file" && state != "none" {
                let said = if probe.output.is_empty() { "no output" } else { &probe.output };
                return fail(format!("Could not tell what {path} is: {said}"));
            }
            state
        }
    };
    if state == "dir" {
        return fail(format!("{path} is a directory, not a file."));
    }
    let exists = state == "file";

    let plan = if request.kind == FileOpKind::Edit {
        plan_edit(request, &path, exists, run).await?
    } else {
        plan_content(request, &path, exists, run).await?
    };

    if plan.bytes > MAX_FILE_BYTES {
        return fail(format!(
            "That is {} bytes and the limit is {MAX_FILE_BYTES}. Write the file in parts with \
             append, or generate it on the machine instead of sending it.",
            plan.bytes
        ));
    }
    Ok(plan)
}

async fn plan_content(
    request: &FileRequest,
    path: &str,
    exists: bool,
    run: &dyn FileRunner,
) -> Result<FilePlan, FileOpError> {
    let payload = end_with_newline(request.content.as_deref().unwrap_or_default());
    if request.kind == FileOpKind::Append && payload.is_empty() {
        return fail("There is nothing to append. Put the text in \"content\".");
    }
    /*
     * Two facts about the file it already is, and one read where there is a
     * channel to read on.
     *
     * A file that is already there keeps its encoding; only a new one is ours to
     * choose, and a new one is UTF-8. Replacing a file is not a licence to change
     * what every other program on that machine reads it as.
     *
     * And a write shows the file whole, so it starts where the file does, while
     * an append shows only the tail being added -- numbering that from 1 would
     * label it with lines belonging to the top of the file. So where the file
     * currently ends has to be known too.
     *
     * Over the shell those are two commands, `iconv` and `wc -l`. Over the
     * channel they are two questions about the same bytes, and the bytes are one
     * read, so they are answered together rather than asked twice.
     */
    let fetched = if exists { run.fetch(path.to_string()).await } else { None };
    let (encoding, known_lines) = match &fetched {
        Some(Ok(data)) => {
            let encoding = detect_encoding(data, false);
            let (text, _) = decode_content(data, encoding);
            // Newlines, not lines -- the same thing for a file that ends in one,
            // and `wc -l` counts the same way, so the two paths agree.
            (encoding, Some(text.matches('\n').count() as u32))
        }
        _ if exists => (probe_encoding(path, run).await, None),
        _ => (FileEncoding::Utf8, None),
    };
    let line = if request.kind == FileOpKind::Append {
        match known_lines {
            Some(lines) => lines + 1,
            None => count_lines(path, exists, run).await + 1,
        }
    } else {
        1
    };
    Ok(FilePlan {
        kind: request.kind,
        path: path.to_string(),
        exists,
        bytes: encode_content(&payload, encoding).len(),
        preview: payload.clone(),
        before: None,
        after: None,
        line: Some(line),
        payload,
        encoding,
    })
}

/// Lines already in the file, for numbering what is about to follow them.
///
/// `wc -l` counts newlines rather than lines, which is the same thing for a file
/// that ends in one -- and every file this writes does, because `end_with_newline`
/// saw to it. A file that will not answer gives 0, and the append is numbered as
/// if from the top: a wrong gutter is worth less than the write, never more.
async fn count_lines(path: &str, exists: bool, run: &dyn FileRunner) -> u32 {
    if !exists {
        return 0;
    }
    let command = format!("wc -l < {} 2>/dev/null | tr -d ' \\n'", quote(path));
    let result = run.run(command, silent()).await;
    if result.exit_code != 0 {
        return 0;
    }
    result.output.trim().parse().unwrap_or(0)
}

async fn probe_encoding(path: &str, run: &dyn FileRunner) -> FileEncoding {
    // With the bytes in hand the encoding is settled rather than guessed at, and
    // `partial: false` says so -- the sample-tail allowance the shell probe needs
    // is for an answer drawn from the first 64 KB, and this is not one.
    if let Some(Ok(data)) = run.fetch(path.to_string()).await {
        return detect_encoding(&data, false);
    }
    let result = run.run(build_encoding_probe(path), silent_raw()).await;
    // An unreadable answer is not worth failing a write over: UTF-8 is what the
    // file would have been written as before any of this existed.
    if result.timed_out || result.exit_code != 0 {
        return FileEncoding::Utf8;
    }
    read_encoding_probe(&result.output)
}

async fn plan_edit(
    request: &FileRequest,
    path: &str,
    exists: bool,
    run: &dyn FileRunner,
) -> Result<FilePlan, FileOpError> {
    let old_text = request.old_text.clone().unwrap_or_default();
    let new_text = request.new_text.clone().unwrap_or_default();
    if !exists {
        return fail(format!(
            "{path} does not exist, so there is nothing to edit. Use write to create it."
        ));
    }
    if old_text.is_empty() {
        return fail("\"old\" is empty. Give the exact text to replace.");
    }

    let (current, encoding) = read_file(path, run).await?;
    let hits = count_occurrences(&current, &old_text);
    if hits == 0 {
        /*
         * The one failure worth diagnosing rather than just reporting.
         *
         * A file is matched against here after being decoded from its own bytes.
         * The model, though, reads files by running `cat`, and that output is
         * decoded with the TERMINAL's encoding. When the two differ -- a UTF-8
         * source file on a GBK terminal is the common one -- every non-ASCII
         * character the model copies out of the terminal is mojibake, and an
         * anchor containing one can never match no matter how carefully it was
         * copied.
         *
         * Left unexplained this costs three or four steps of the model trying the
         * same thing in different disguises, which is exactly what it did.
         */
        let anchor_non_ascii = !old_text.is_ascii();
        let extra = if anchor_non_ascii {
            format!(
                " Your \"old\" contains non-ASCII characters. Terminal output is decoded with the \
                 terminal's encoding and this file is {}, so anything you copied out of a cat may \
                 not be the bytes that are actually in the file. Anchor on a line of pure ASCII -- \
                 code rather than a comment -- and it will match.",
                match encoding {
                    FileEncoding::Utf8 => "utf8",
                    FileEncoding::Gb18030 => "gb18030",
                }
            )
        } else {
            String::new()
        };
        return fail(format!(
            "That text does not appear in {path}. Read the file first and copy the fragment \
             exactly, whitespace included.{extra}"
        ));
    }
    if hits > 1 {
        return fail(format!(
            "That text appears {hits} times in {path}, so which one to change is ambiguous. Give \
             a longer fragment that occurs exactly once."
        ));
    }

    let at = current.find(&old_text).unwrap_or(0);
    let payload = current.replacen(&old_text, &new_text, 1);
    // Counted off the text before the match, which is the whole of what decides
    // the number: the newlines in front of the fragment are the lines above it.
    let line = current[..at].split('\n').count() as u32;
    Ok(FilePlan {
        kind: FileOpKind::Edit,
        path: path.to_string(),
        exists,
        bytes: encode_content(&payload, encoding).len(),
        preview: payload.clone(),
        before: Some(old_text),
        after: Some(new_text),
        line: Some(line),
        payload,
        encoding,
    })
}

/// Reads the file back as base64, which is exact where `cat` is not: no encoding
/// to guess at, and no chance of the terminal treating a byte in the file as
/// control.
async fn read_file(
    path: &str,
    run: &dyn FileRunner,
) -> Result<(String, FileEncoding), FileOpError> {
    let data = match run.fetch(path.to_string()).await {
        Some(Ok(data)) => data,
        // The channel is there and it said no. Not something to paper over with
        // the shell: a read that failed over SFTP failed for a reason the shell
        // would meet too -- no such file, no permission -- and trying twice would
        // only report the second machine's words for the first machine's problem.
        Some(Err(why)) => return fail(format!("Could not read {path}: {why}")),
        None => {
            let result = run.run(build_read(path), silent_raw()).await;
            if result.timed_out {
                return fail(format!("Reading {path} was interrupted before it finished."));
            }
            if result.exit_code != 0 {
                let said = if result.output.is_empty() { "no output" } else { &result.output };
                return fail(format!("Could not read {path}: {said}"));
            }
            let packed: String =
                result.output.chars().filter(|c| !c.is_whitespace()).collect();
            base64::engine::general_purpose::STANDARD.decode(&packed).unwrap_or_default()
        }
    };
    if data.len() > MAX_FILE_BYTES {
        return fail(format!(
            "{path} is {} bytes, too large to edit this way. Change it with a command instead.",
            data.len()
        ));
    }
    // The whole file is in hand here, so the encoding is settled exactly rather
    // than probed. What must still be refused is a file that is not text at all:
    // decoding an executable produces something an edit could appear to succeed on.
    if looks_binary(&data) {
        return fail(format!("{path} is not a text file, so it cannot be edited this way."));
    }
    let encoding = detect_encoding(&data, false);
    let (text, had_errors) = decode_content(&data, encoding);
    if had_errors || text.contains('\u{fffd}') {
        return fail(format!(
            "{path} is neither UTF-8 nor GBK text, so it cannot be edited this way. Convert it \
             first, or change it with a command."
        ));
    }
    Ok((text, encoding))
}

/// Carries out a plan the user has agreed to.
///
/// The result is shaped like any other command's, because to the loop above this
/// is one step among the others: it succeeded or it did not, and what it printed
/// is what the model gets to read.
pub async fn apply_file_op(plan: &FilePlan, run: &dyn FileRunner) -> CommandResult {
    let append = plan.kind == FileOpKind::Append;
    let nonce = format!("{:x}", now_nanos());
    let temp = temp_path_for(&plan.path, &nonce);
    let label = format!("# tshell {} {} ({})", plan.kind.tag(), plan.path, format_bytes(plan.bytes));

    let bytes = encode_content(&plan.payload, plan.encoding);

    /*
     * Over the channel the payload is one write, and the shell is left with the
     * single step that has to be atomic and the single step worth watching.
     *
     * Both kinds stage to the same temporary here, where the typed path staged
     * only a write and appended straight onto the target. Uniform on purpose: an
     * append whose bytes were half delivered would otherwise be half appended,
     * and there would be nothing to take back.
     */
    let commands = match run.stash(temp.clone(), bytes.clone()).await {
        Some(Ok(())) => vec![if append {
            build_append_commit(&plan.path, &temp)
        } else {
            build_commit(&plan.path, &temp, plan.exists)
        }],
        Some(Err(why)) => {
            return CommandResult {
                output: format!(
                    "{} of {} failed and nothing was changed.\nThe bytes could not be sent: {why}",
                    plan.kind.tag(),
                    plan.path
                ),
                exit_code: 1,
                timed_out: false,
                truncated: false,
            };
        }
        // No channel. The bytes go the way they always did -- as base64, a line
        // at a time -- and an append still lands straight on the target, because
        // staging it would buy nothing without a single write to stage.
        None => {
            let target = if append { plan.path.clone() } else { temp.clone() };
            let mut commands = build_chunk_commands(&bytes, &target, append);
            if !append {
                commands.push(build_commit(&plan.path, &temp, plan.exists));
            }
            commands
        }
    };

    for (index, command) in commands.into_iter().enumerate() {
        // The first step carries the label, so the terminal shows the intent once
        // rather than a column of base64 the user cannot read anyway.
        let options = if index == 0 {
            RunOptions { display: Some(label.clone()), ..Default::default() }
        } else {
            silent()
        };
        let result = run.run(command, options).await;
        if !failed(&result) {
            continue;
        }
        if !append {
            run.run(format!("rm -f -- {}", quote(&temp)), silent()).await;
        }
        let interrupted =
            if result.timed_out { "The step was interrupted after the timeout.\n" } else { "" };
        return CommandResult {
            output: format!(
                "{} of {} failed and nothing was changed.\n{interrupted}{}",
                plan.kind.tag(),
                plan.path,
                result.output
            ),
            ..result
        };
    }

    verify(plan, run).await
}

async fn verify(plan: &FilePlan, run: &dyn FileRunner) -> CommandResult {
    let check = run.run(build_verify(&plan.path), silent()).await;
    let mut fields = check.output.trim().split_whitespace();
    let size = fields.next().unwrap_or_default().to_string();
    let digest = fields.next().unwrap_or_default().to_string();
    let expected = sha256(&encode_content(&plan.payload, plan.encoding));

    // A whole-file write is checkable against what was sent; an append is not, so
    // its size is reported rather than judged.
    if plan.kind != FileOpKind::Append && !digest.is_empty() && digest != expected {
        return CommandResult {
            output: format!(
                "{} was written but its checksum does not match what was sent. Read it back and \
                 check it.",
                plan.path
            ),
            exit_code: 1,
            timed_out: false,
            truncated: false,
        };
    }

    /*
     * An edit reports the change, not the file.
     *
     * All three kinds land the same way -- the whole file is sent back down -- so
     * an edit used to be described in the write's words: "Wrote 3020 bytes to
     * btree.c, replacing what was there." Mechanically true and thoroughly
     * misleading. The model had replaced forty bytes in the middle of a function,
     * and was being told it had rewritten three kilobytes over the top of whatever
     * was there before. Two things follow from believing that, and both are bad:
     * it stops trusting that an edit is surgical, and the prompt's own rule --
     * "you have just been told how many bytes it holds, so you know what is in it"
     * -- starts reasoning from a number that describes the wrong thing.
     *
     * So the sizes reported are the sizes of the fragment that moved, and the
     * whole-file figure stays where it belongs, in the sentence about the file.
     */
    let fragment = |text: &Option<String>| {
        encode_content(text.as_deref().unwrap_or_default(), plan.encoding).len()
    };
    let wrote = match plan.kind {
        FileOpKind::Append => format!("Appended {} bytes to {}.", plan.bytes, plan.path),
        FileOpKind::Edit => {
            let at = match plan.line {
                Some(line) => format!(" at line {line}"),
                None => String::new(),
            };
            format!(
                "Edited {}: replaced {} bytes with {}{at}. Nothing else in the file was touched.",
                plan.path,
                fragment(&plan.before),
                fragment(&plan.after)
            )
        }
        FileOpKind::Write => {
            let replacing = if plan.exists { ", replacing what was there" } else { "" };
            format!("Wrote {} bytes to {}{replacing}.", plan.bytes, plan.path)
        }
    };
    /*
     * Said out loud, because a model that is not told assumes UTF-8 and reaches
     * for iconv to "fix" a file that was never broken -- and its usual one-liner
     * only converts one way, which leaves the file in the encoding it was trying
     * to preserve. tshell handles this end to end; the model's job is to leave it
     * be.
     */
    let encoded = if plan.encoding == FileEncoding::Gb18030 {
        "It was written in GBK/GB18030, the encoding the file already had, so everything else that \
         reads it still can. Do not convert it with iconv or anything else."
    } else {
        ""
    };
    let sized = if size.is_empty() { String::new() } else { format!("The file is now {size} bytes.") };
    let output = [wrote.as_str(), sized.as_str(), encoded]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    CommandResult { output, exit_code: 0, timed_out: false, truncated: false }
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A machine that answers from a table, so a plan can be tested without one.
    #[derive(Default)]
    struct Fake {
        answers: Mutex<Vec<CommandResult>>,
        seen: Mutex<Vec<String>>,
    }

    impl Fake {
        fn with(answers: Vec<CommandResult>) -> Self {
            Self { answers: Mutex::new(answers), seen: Mutex::new(Vec::new()) }
        }
        fn ok(output: &str) -> CommandResult {
            CommandResult { output: output.into(), ..Default::default() }
        }
        fn commands(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    impl FileRunner for Fake {
        fn run<'a>(
            &'a self,
            command: String,
            _options: RunOptions,
        ) -> Pin<Box<dyn Future<Output = CommandResult> + Send + 'a>> {
            self.seen.lock().unwrap().push(command);
            let mut answers = self.answers.lock().unwrap();
            let answer =
                if answers.is_empty() { CommandResult::default() } else { answers.remove(0) };
            Box::pin(async move { answer })
        }
    }

    /// A machine that also has the assistant's own connection.
    ///
    /// Wraps the plain `Fake` rather than replacing it, so a test can assert on
    /// what still went through the shell -- which is the whole point of the
    /// second channel: almost nothing should.
    struct Wired {
        shell: Fake,
        files: Mutex<std::collections::HashMap<String, Vec<u8>>>,
        /// Set when the channel is there but refuses. Not the same as absent.
        refuse: bool,
    }

    impl Wired {
        fn with(files: &[(&str, &[u8])]) -> Self {
            Self {
                shell: Fake::default(),
                files: Mutex::new(
                    files.iter().map(|(p, b)| (p.to_string(), b.to_vec())).collect(),
                ),
                refuse: false,
            }
        }
        fn commands(&self) -> Vec<String> {
            self.shell.commands()
        }
        fn written(&self, path: &str) -> Option<Vec<u8>> {
            self.files.lock().unwrap().get(path).cloned()
        }
    }

    impl FileRunner for Wired {
        fn run<'a>(
            &'a self,
            command: String,
            options: RunOptions,
        ) -> Pin<Box<dyn Future<Output = CommandResult> + Send + 'a>> {
            self.shell.run(command, options)
        }

        fn fetch<'a>(
            &'a self,
            path: String,
        ) -> Pin<Box<dyn Future<Output = Option<Result<Vec<u8>, String>>> + Send + 'a>> {
            let answer = if self.refuse {
                Some(Err("permission denied".to_string()))
            } else {
                self.files
                    .lock()
                    .unwrap()
                    .get(&path)
                    .cloned()
                    .map(Ok)
                    .or(Some(Err("no such file".to_string())))
            };
            Box::pin(async move { answer })
        }

        fn stash<'a>(
            &'a self,
            path: String,
            bytes: Vec<u8>,
        ) -> Pin<Box<dyn Future<Output = Option<Result<(), String>>> + Send + 'a>> {
            if self.refuse {
                return Box::pin(async { Some(Err("read-only file system".to_string())) });
            }
            self.files.lock().unwrap().insert(path, bytes);
            Box::pin(async { Some(Ok(())) })
        }

        fn probe<'a>(
            &'a self,
            path: String,
        ) -> Pin<Box<dyn Future<Output = Option<Presence>> + Send + 'a>> {
            let here = self.files.lock().unwrap().contains_key(&path);
            Box::pin(async move {
                Some(if here { Presence::File } else { Presence::Absent })
            })
        }
    }

    fn base64_of(text: &str) -> String {
        base64::engine::general_purpose::STANDARD.encode(text.as_bytes())
    }

    fn write_request(path: &str, content: &str) -> FileRequest {
        FileRequest {
            kind: FileOpKind::Write,
            path: path.into(),
            content: Some(content.into()),
            old_text: None,
            new_text: None,
        }
    }

    #[test]
    fn a_temporary_lands_beside_its_target() {
        assert_eq!(temp_path_for("/etc/hosts", "ab"), "/etc/.tshell-ab.tmp");
        assert_eq!(temp_path_for("/hosts", "ab"), "/.tshell-ab.tmp");
        assert_eq!(temp_path_for("hosts", "ab"), ".tshell-ab.tmp");
        assert_eq!(dir_of("/a/b/c"), "/a/b");
        assert_eq!(dir_of("/c"), "/");
        assert_eq!(dir_of("c"), "");
    }

    #[test]
    fn content_travels_as_base64_in_chunks_that_decode_on_their_own() {
        let payload = "x".repeat(5000);
        let chunks = base64_chunks(payload.as_bytes(), CHUNK_CHARS);
        assert!(chunks.len() > 1);
        assert_eq!(chunks[0].len() % 4, 0, "a chunk boundary must be a base64 boundary");

        let commands = build_chunk_commands(payload.as_bytes(), "/tmp/x", false);
        assert!(commands[0].contains("base64 -d > '/tmp/x'"));
        assert!(commands[1].contains("base64 -d >> '/tmp/x'"));
    }

    #[test]
    fn an_append_of_nothing_writes_nothing_and_a_write_of_nothing_truncates() {
        assert!(build_chunk_commands(b"", "/tmp/x", true).is_empty());
        assert_eq!(build_chunk_commands(b"", "/tmp/x", false), vec![": > '/tmp/x'"]);
    }

    #[test]
    fn encodings_round_trip_and_are_told_apart() {
        let text = "你好 world";
        let utf8 = encode_content(text, FileEncoding::Utf8);
        let gbk = encode_content(text, FileEncoding::Gb18030);
        assert_ne!(utf8, gbk);
        assert_eq!(decode_content(&utf8, FileEncoding::Utf8).0, text);
        assert_eq!(decode_content(&gbk, FileEncoding::Gb18030).0, text);

        assert_eq!(detect_encoding(&utf8, false), FileEncoding::Utf8);
        assert_eq!(detect_encoding(&gbk, false), FileEncoding::Gb18030);
        assert_eq!(detect_encoding(b"plain ascii", false), FileEncoding::Utf8);
    }

    #[test]
    fn a_sample_cut_through_a_character_is_not_read_as_another_encoding() {
        let full = "你好".as_bytes();
        let cut = &full[..full.len() - 1];
        assert_eq!(detect_encoding(cut, false), FileEncoding::Gb18030, "the raw sample lies");
        assert_eq!(detect_encoding(cut, true), FileEncoding::Utf8, "trimming it tells the truth");
    }

    #[test]
    fn the_encoding_probe_reads_both_of_its_answers() {
        assert_eq!(read_encoding_probe(&format!("noise\n{UTF8_MARKER}")), FileEncoding::Utf8);
        assert_eq!(read_encoding_probe(&format!("noise\n{OTHER_MARKER}")), FileEncoding::Gb18030);
        assert_eq!(read_encoding_probe(&base64_of("plain")), FileEncoding::Utf8);

        let gbk = base64::engine::general_purpose::STANDARD
            .encode(encode_content("你好", FileEncoding::Gb18030));
        assert_eq!(read_encoding_probe(&gbk), FileEncoding::Gb18030);
    }

    #[test]
    fn occurrences_are_counted_without_overlapping() {
        assert_eq!(count_occurrences("aaaa", "aa"), 2);
        assert_eq!(count_occurrences("abc", "z"), 0);
        assert_eq!(count_occurrences("abc", ""), 0);
    }

    #[test]
    fn a_commit_keeps_the_mode_of_the_file_it_replaces() {
        assert!(build_commit("/etc/x", "/etc/.t", true).starts_with("chmod --reference="));
        assert_eq!(build_commit("/etc/x", "/etc/.t", false), "mv -f -- '/etc/.t' '/etc/x'");
    }

    #[tokio::test]
    async fn a_write_to_a_new_file_is_utf8_and_starts_at_line_one() {
        let fake = Fake::with(vec![Fake::ok("none")]);
        let plan = plan_file_op(&write_request("/tmp/new.txt", "hello"), &fake).await.unwrap();
        assert!(!plan.exists);
        assert_eq!(plan.encoding, FileEncoding::Utf8);
        assert_eq!(plan.payload, "hello\n", "a text file ends in a newline");
        assert_eq!(plan.line, Some(1));
        assert_eq!(plan.bytes, 6);
    }

    #[tokio::test]
    async fn a_write_over_an_existing_file_keeps_the_encoding_it_had() {
        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(OTHER_MARKER)]);
        let plan = plan_file_op(&write_request("/tmp/old.txt", "你好"), &fake).await.unwrap();
        assert!(plan.exists);
        assert_eq!(plan.encoding, FileEncoding::Gb18030);
    }

    #[tokio::test]
    async fn a_directory_is_refused_before_anything_is_read() {
        let fake = Fake::with(vec![Fake::ok("dir")]);
        let error = plan_file_op(&write_request("/tmp", "x"), &fake).await.unwrap_err();
        assert!(error.0.contains("is a directory"));

        let fake = Fake::default();
        let error = plan_file_op(&write_request("/tmp/", "x"), &fake).await.unwrap_err();
        assert!(error.0.contains("names a directory"));
        assert!(fake.commands().is_empty(), "nothing is asked of the machine");
    }

    #[tokio::test]
    async fn an_append_is_numbered_on_from_where_the_file_ends() {
        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(UTF8_MARKER), Fake::ok("12")]);
        let request = FileRequest {
            kind: FileOpKind::Append,
            path: "/tmp/log".into(),
            content: Some("one more".into()),
            old_text: None,
            new_text: None,
        };
        let plan = plan_file_op(&request, &fake).await.unwrap();
        assert_eq!(plan.line, Some(13));
    }

    #[tokio::test]
    async fn an_edit_finds_its_fragment_and_says_which_line_it_is_on() {
        let file = "alpha\nbravo\ncharlie\n";
        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(&base64_of(file))]);
        let request = FileRequest {
            kind: FileOpKind::Edit,
            path: "/tmp/x".into(),
            content: None,
            old_text: Some("bravo".into()),
            new_text: Some("BRAVO".into()),
        };
        let plan = plan_file_op(&request, &fake).await.unwrap();
        assert_eq!(plan.payload, "alpha\nBRAVO\ncharlie\n");
        assert_eq!(plan.line, Some(2));
        assert_eq!(plan.before.as_deref(), Some("bravo"));
        assert_eq!(plan.after.as_deref(), Some("BRAVO"));
    }

    #[tokio::test]
    async fn an_edit_that_matches_nothing_or_twice_says_which() {
        let file = "same\nsame\n";
        let request = |old: &str| FileRequest {
            kind: FileOpKind::Edit,
            path: "/tmp/x".into(),
            content: None,
            old_text: Some(old.into()),
            new_text: Some("new".into()),
        };

        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(&base64_of(file))]);
        let error = plan_file_op(&request("same"), &fake).await.unwrap_err();
        assert!(error.0.contains("appears 2 times"), "{}", error.0);

        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(&base64_of(file))]);
        let error = plan_file_op(&request("absent"), &fake).await.unwrap_err();
        assert!(error.0.contains("does not appear"));
        assert!(!error.0.contains("non-ASCII"), "the anchor was ASCII");
    }

    #[tokio::test]
    async fn a_non_ascii_anchor_that_missed_is_told_why_it_might_have() {
        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(&base64_of("plain\n"))]);
        let request = FileRequest {
            kind: FileOpKind::Edit,
            path: "/tmp/x".into(),
            content: None,
            old_text: Some("注释".into()),
            new_text: Some("x".into()),
        };
        let error = plan_file_op(&request, &fake).await.unwrap_err();
        assert!(error.0.contains("non-ASCII characters"));
        assert!(error.0.contains("Anchor on a line of pure ASCII"));
    }

    #[tokio::test]
    async fn editing_a_file_that_is_not_there_points_at_write() {
        let fake = Fake::with(vec![Fake::ok("none")]);
        let request = FileRequest {
            kind: FileOpKind::Edit,
            path: "/tmp/x".into(),
            content: None,
            old_text: Some("a".into()),
            new_text: Some("b".into()),
        };
        let error = plan_file_op(&request, &fake).await.unwrap_err();
        assert!(error.0.contains("Use write to create it"));
    }

    #[tokio::test]
    async fn a_binary_file_is_refused_rather_than_half_edited() {
        let binary = base64::engine::general_purpose::STANDARD.encode([0x7f, 0x45, 0x00, 0x01]);
        let fake = Fake::with(vec![Fake::ok("file"), Fake::ok(&binary)]);
        let request = FileRequest {
            kind: FileOpKind::Edit,
            path: "/tmp/x".into(),
            content: None,
            old_text: Some("a".into()),
            new_text: Some("b".into()),
        };
        let error = plan_file_op(&request, &fake).await.unwrap_err();
        assert!(error.0.contains("not a text file"));
    }

    #[tokio::test]
    async fn something_too_big_to_paste_is_refused_with_the_limit() {
        let fake = Fake::with(vec![Fake::ok("none")]);
        let huge = "x".repeat(MAX_FILE_BYTES + 1);
        let error = plan_file_op(&write_request("/tmp/x", &huge), &fake).await.unwrap_err();
        assert!(error.0.contains(&MAX_FILE_BYTES.to_string()));
        assert!(error.0.contains("append"));
    }

    #[tokio::test]
    async fn applying_a_write_stages_it_and_moves_it_into_place() {
        let plan = FilePlan {
            kind: FileOpKind::Write,
            path: "/tmp/x".into(),
            exists: true,
            bytes: 6,
            preview: "hello\n".into(),
            before: None,
            after: None,
            line: Some(1),
            payload: "hello\n".into(),
            encoding: FileEncoding::Utf8,
        };
        let digest = sha256(b"hello\n");
        let fake = Fake::with(vec![Fake::ok(""), Fake::ok(""), Fake::ok(&format!("6 {digest}"))]);

        let result = apply_file_op(&plan, &fake).await;
        assert_eq!(result.exit_code, 0);
        assert!(result.output.starts_with("Wrote 6 bytes to /tmp/x, replacing what was there."));
        assert!(result.output.contains("The file is now 6 bytes."));

        let commands = fake.commands();
        assert!(commands[0].contains(".tshell-"), "staged beside the target");
        assert!(commands[1].starts_with("chmod --reference="));
        assert!(commands[1].contains("mv -f --"));
    }

    #[tokio::test]
    async fn a_checksum_that_does_not_match_is_reported_as_a_failure() {
        let plan = FilePlan {
            kind: FileOpKind::Write,
            path: "/tmp/x".into(),
            exists: false,
            bytes: 6,
            preview: "hello\n".into(),
            before: None,
            after: None,
            line: Some(1),
            payload: "hello\n".into(),
            encoding: FileEncoding::Utf8,
        };
        let fake = Fake::with(vec![Fake::ok(""), Fake::ok(""), Fake::ok("6 deadbeef")]);
        let result = apply_file_op(&plan, &fake).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.output.contains("checksum does not match"));
    }

    #[tokio::test]
    async fn a_failed_chunk_removes_the_staging_file_and_changes_nothing() {
        let plan = FilePlan {
            kind: FileOpKind::Write,
            path: "/tmp/x".into(),
            exists: true,
            bytes: 6,
            preview: "hello\n".into(),
            before: None,
            after: None,
            line: Some(1),
            payload: "hello\n".into(),
            encoding: FileEncoding::Utf8,
        };
        let broken =
            CommandResult { output: "no space".into(), exit_code: 1, ..Default::default() };
        let fake = Fake::with(vec![broken]);

        let result = apply_file_op(&plan, &fake).await;
        assert_eq!(result.exit_code, 1);
        assert!(result.output.starts_with("write of /tmp/x failed and nothing was changed."));
        assert!(fake.commands().iter().any(|command| command.starts_with("rm -f --")));
    }

    #[tokio::test]
    async fn an_edit_reports_the_fragment_that_moved_rather_than_the_file() {
        let plan = FilePlan {
            kind: FileOpKind::Edit,
            path: "/tmp/x".into(),
            exists: true,
            bytes: 3000,
            preview: String::new(),
            before: Some("old".into()),
            after: Some("a longer new one".into()),
            line: Some(42),
            payload: "whole file".into(),
            encoding: FileEncoding::Utf8,
        };
        let digest = sha256(b"whole file");
        let fake = Fake::with(vec![Fake::ok(""), Fake::ok(""), Fake::ok(&format!("3000 {digest}"))]);

        let result = apply_file_op(&plan, &fake).await;
        assert!(result.output.contains("replaced 3 bytes with 16 at line 42"));
        assert!(result.output.contains("Nothing else in the file was touched."));
        assert!(!result.output.contains("3000 bytes to"));
    }

    #[tokio::test]
    async fn a_gbk_file_says_so_and_tells_the_model_to_leave_it_alone() {
        let payload = "你好\n";
        let bytes = encode_content(payload, FileEncoding::Gb18030);
        let plan = FilePlan {
            kind: FileOpKind::Write,
            path: "/tmp/x".into(),
            exists: true,
            bytes: bytes.len(),
            preview: payload.into(),
            before: None,
            after: None,
            line: Some(1),
            payload: payload.into(),
            encoding: FileEncoding::Gb18030,
        };
        let digest = sha256(&bytes);
        let fake = Fake::with(vec![Fake::ok(""), Fake::ok(""), Fake::ok(&format!("5 {digest}"))]);

        let result = apply_file_op(&plan, &fake).await;
        assert!(result.output.contains("GBK/GB18030"));
        assert!(result.output.contains("Do not convert it with iconv"));
    }

    #[test]
    fn sizes_read_the_way_a_person_would_say_them() {
        assert_eq!(format_bytes(6), "6 B");
        assert_eq!(format_bytes(2048), "2.0 KB");
    }

    // ------------------------------------------------- the assistant's own channel ---

    /// The whole point, stated as a number. A file that used to be forty-odd
    /// typed commands is one write and one `mv`.
    #[tokio::test]
    async fn a_write_over_the_channel_costs_one_shell_command() {
        let big = "x".repeat(100 * 1024);
        let wired = Wired::with(&[]);
        let request = FileRequest {
            kind: FileOpKind::Write,
            path: "/etc/app.conf".into(),
            content: Some(big.clone()),
            old_text: None,
            new_text: None,
        };

        let plan = plan_file_op(&request, &wired).await.unwrap();
        apply_file_op(&plan, &wired).await;

        let commands = wired.commands();
        // The commit, and the verify that follows it. Nothing else.
        assert_eq!(commands.len(), 2, "commands were: {commands:#?}");
        assert!(commands[0].contains("mv -f --"));
        assert!(commands[1].starts_with("wc -c <"));
        assert!(
            !commands.iter().any(|command| command.contains("base64")),
            "no byte should have been typed"
        );
    }

    /// The staged bytes are the file, not a description of it.
    #[tokio::test]
    async fn the_staged_temporary_holds_exactly_what_was_planned() {
        let wired = Wired::with(&[]);
        let request = FileRequest {
            kind: FileOpKind::Write,
            path: "/srv/x.conf".into(),
            content: Some("hello
".into()),
            old_text: None,
            new_text: None,
        };
        let plan = plan_file_op(&request, &wired).await.unwrap();
        apply_file_op(&plan, &wired).await;

        let staged = wired
            .commands()
            .first()
            .and_then(|commit| {
                let start = commit.find("/srv/.tshell-")?;
                let end = commit[start..].find(".tmp")? + start + 4;
                Some(commit[start..end].to_string())
            })
            .expect("the commit names the temporary");
        assert_eq!(wired.written(&staged).unwrap(), b"hello
");
    }

    /// An append stages too, and commits with `cat` rather than a move: there is
    /// a file to land at the end of, and a move would replace it.
    #[tokio::test]
    async fn an_append_over_the_channel_lands_at_the_end() {
        let wired = Wired::with(&[("/var/log/notes", b"first
")]);
        let request = FileRequest {
            kind: FileOpKind::Append,
            path: "/var/log/notes".into(),
            content: Some("second
".into()),
            old_text: None,
            new_text: None,
        };
        let plan = plan_file_op(&request, &wired).await.unwrap();
        apply_file_op(&plan, &wired).await;

        // The only shell command in the whole operation, planning included: the
        // line count came off the bytes that were already in hand.
        let commands = wired.commands();
        assert_eq!(commands.len(), 2, "commands were: {commands:#?}");
        let commit = &commands[0];
        assert!(commit.contains(">> '/var/log/notes'"), "commit was: {commit}");
        assert!(commit.contains("rm -f --"), "the staged bytes are cleaned up");
        assert!(!commit.contains("mv -f"), "an append must not replace the file");
    }

    /// The fallback is not a broken path, it is the path a host without a second
    /// connection takes -- and it has to still be the one that worked before.
    #[tokio::test]
    async fn without_a_channel_the_bytes_are_still_typed() {
        let fake = Fake::with(vec![Fake::ok("none")]);
        let request = FileRequest {
            kind: FileOpKind::Write,
            path: "/tmp/a.conf".into(),
            content: Some("hello
".into()),
            old_text: None,
            new_text: None,
        };
        let plan = plan_file_op(&request, &fake).await.unwrap();
        apply_file_op(&plan, &fake).await;

        let commands = fake.commands();
        assert!(commands.iter().any(|command| command.contains("base64 -d >")));
        assert!(commands.iter().any(|command| command.contains("mv -f --")));
    }

    /// A channel that answered "no" is answering about the file. Asking the shell
    /// the same question would only report a second machine's words for the first
    /// machine's refusal.
    #[tokio::test]
    async fn a_refusal_from_the_channel_is_not_retried_through_the_shell() {
        let mut wired = Wired::with(&[("/etc/shadow", b"root:x
")]);
        wired.refuse = true;
        let request = FileRequest {
            kind: FileOpKind::Edit,
            path: "/etc/shadow".into(),
            content: None,
            old_text: Some("root:x".into()),
            new_text: Some("root:y".into()),
        };

        let failure = plan_file_op(&request, &wired).await.unwrap_err();
        assert!(failure.0.contains("permission denied"), "said: {}", failure.0);
        assert!(
            !wired.commands().iter().any(|command| command.starts_with("base64 ")),
            "the shell must not have been asked to read it too"
        );
    }

    /// With the bytes in hand the encoding is settled rather than sampled, so the
    /// probe command never goes out.
    #[tokio::test]
    async fn the_encoding_is_read_off_the_bytes_not_probed_for() {
        let gbk = encode_content("\u{4f60}\u{597d}\n", FileEncoding::Gb18030);
        let wired = Wired::with(&[("/srv/gbk.txt", &gbk)]);
        let request = FileRequest {
            kind: FileOpKind::Append,
            path: "/srv/gbk.txt".into(),
            content: Some("more\n".into()),
            old_text: None,
            new_text: None,
        };

        let plan = plan_file_op(&request, &wired).await.unwrap();
        assert_eq!(plan.encoding, FileEncoding::Gb18030, "the file keeps what it had");
        assert!(
            !wired.commands().iter().any(|command| command.contains("iconv")),
            "nothing needed probing"
        );
    }

    #[test]
    fn an_append_commit_reports_the_copy_and_not_the_cleanup() {
        let commit = build_append_commit("/a/b", "/a/.tshell-1.tmp");
        // `rm` runs either way, and its own status is discarded: a successful
        // append followed by a failed cleanup is a successful append.
        let cat = commit.find("cat --").unwrap();
        let status = commit.find("status=$?").unwrap();
        let remove = commit.find("rm -f --").unwrap();
        assert!(cat < status && status < remove);
        assert!(commit.ends_with("exit $status"));
    }
}
