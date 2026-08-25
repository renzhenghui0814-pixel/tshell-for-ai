//! The two panes of the transfer page: the local disk and an SFTP session.
//!
//! Both sides answer the same questions -- list a directory, stat a path, make
//! one, remove one, rename one -- and [`Source`] is where that is written down
//! once. The extension had the same shape as a `FileSource` interface, and for
//! the same reason: a transfer is always "read from one side, write to the
//! other", so the code that moves bytes should not know which way it is going.
//!
//! It is an enum rather than a trait object. The two implementations are known
//! and there will not be a third, `async fn` in traits is not dyn-compatible
//! without boxing every call, and matching twice is cheaper to read than the
//! machinery that would avoid it.
//!
//! The remote side keeps its own SSH connection, separate from the terminal's.
//! SFTP is a subsystem on its own channel and could have shared one, but then a
//! listing would queue behind whatever the shell is doing, and closing either
//! tab would have to work out whether the other still needed the connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use russh_sftp::client::SftpSession;
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};

use crate::ssh;

/// One row of a pane. The field names are the page's, which is the extension's.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub size: u64,
    /// Milliseconds since the epoch, because that is what a `Date` takes.
    pub modified_at: i64,
}

/// Directories first, then by name -- the order the extension's `compareEntries`
/// produced, and the one that makes a pane readable rather than merely sorted.
fn order(entries: &mut [Entry]) {
    entries.sort_by(|a, b| {
        let a_dir = a.kind == "directory";
        let b_dir = b.kind == "directory";
        b_dir.cmp(&a_dir).then_with(|| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                .then_with(|| a.name.cmp(&b.name))
        })
    });
}

// ------------------------------------------------------------------ local ---

/// Only what the preview needs. The conflict check that wants a kind and a
/// timestamp arrives with the transfer step, and adds them then.
pub struct Stat {
    pub size: u64,
    /// Seconds since the epoch, or zero when the side cannot report one.
    ///
    /// Zero is not a time, and the editor reads it that way: a server whose
    /// `stat` has no mtime falls back to comparing sizes rather than treating
    /// every save as a conflict.
    pub modified: u64,
}

pub struct Local;

impl Local {
    /*
     * An empty path is not a missing path here: it is the drive list, which is
     * what going up from `C:\` lands on. Unix has a single root and so never
     * produces one.
     */
    fn list(&self, dir: &str, hidden: bool) -> Result<Vec<Entry>, String> {
        if dir.is_empty() {
            return Ok(drives());
        }

        let mut entries = Vec::new();
        let reader = std::fs::read_dir(dir).map_err(|error| error.to_string())?;
        for item in reader.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if !hidden && name.starts_with('.') {
                continue;
            }
            let path = item.path();
            // A broken link or a directory we may not stat still belongs in the
            // list; it is something the user can see and act on.
            let (kind, size, modified_at) = match std::fs::metadata(&path) {
                Ok(meta) => (
                    if meta.is_dir() { "directory" } else { "file" },
                    meta.len(),
                    millis(meta.modified().ok()),
                ),
                Err(_) => ("file", 0, 0),
            };
            entries.push(Entry {
                name,
                path: path.to_string_lossy().into_owned(),
                kind,
                size,
                modified_at,
            });
        }
        order(&mut entries);
        Ok(entries)
    }

    fn stat(&self, path: &str) -> Result<Stat, String> {
        let meta = std::fs::metadata(path).map_err(|error| error.to_string())?;
        let modified = meta
            .modified()
            .ok()
            .and_then(|when| when.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|since| since.as_secs())
            .unwrap_or(0);
        Ok(Stat {
            size: meta.len(),
            modified,
        })
    }

    /*
     * The whole file, replaced in one step.
     *
     * `atomic` rather than a plain write for the reason that module exists: an
     * interrupted overwrite leaves whatever prefix made it out, and for a config
     * file that is every server the user had. The remote side cannot do this and
     * says so at its own definition -- the two are deliberately not the same,
     * because pretending they were would mean claiming a guarantee SFTP v3
     * cannot give.
     */
    fn write_bytes(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        crate::atomic::write_bytes(Path::new(path), bytes).map_err(|error| error.to_string())
    }

    /// A window of a file, for the preview panel. Short reads at the end of the
    /// file are not an error -- they are how the caller learns it has finished.
    fn read_bytes(&self, path: &str, offset: u64, max: usize) -> Result<Vec<u8>, String> {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| error.to_string())?;
        let mut buffer = vec![0u8; max];
        let mut filled = 0;
        while filled < max {
            match file.read(&mut buffer[filled..]) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) => return Err(error.to_string()),
            }
        }
        buffer.truncate(filled);
        Ok(buffer)
    }

    fn mkdir(&self, path: &str) -> Result<(), String> {
        std::fs::create_dir_all(path).map_err(|error| error.to_string())
    }

    fn remove(&self, path: &str) -> Result<(), String> {
        let meta = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
        if meta.is_dir() {
            std::fs::remove_dir_all(path)
        } else {
            std::fs::remove_file(path)
        }
        .map_err(|error| error.to_string())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        std::fs::rename(from, to).map_err(|error| error.to_string())
    }
}

/// The drives that answer, which is the top of the local tree on Windows.
fn drives() -> Vec<Entry> {
    let mut entries = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if Path::new(&root).exists() {
            entries.push(Entry {
                name: root.clone(),
                path: root,
                kind: "directory",
                size: 0,
                modified_at: 0,
            });
        }
    }
    entries
}

fn millis(time: Option<std::time::SystemTime>) -> i64 {
    time.and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

// ----------------------------------------------------------------- remote ---

pub struct Remote {
    /// Reached directly by the copy loop, which streams rather than going
    /// through `read_bytes` -- one open per 64KB block would spend the whole
    /// transfer in round trips.
    sftp: SftpSession,
    /// Keeps the connection open -- dropping it closes the session out from
    /// under the SFTP subsystem -- and answers whether it is still alive.
    handle: russh::client::Handle<ssh::Client>,
}

impl Remote {
    pub async fn open(target: &ssh::Target) -> Result<Self, String> {
        let handle = ssh::connect(target).await?;
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|error| error.to_string())?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|error| error.to_string())?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Remote { sftp, handle })
    }

    fn is_closed(&self) -> bool {
        self.handle.is_closed()
    }

    async fn list(&self, dir: &str, hidden: bool) -> Result<Vec<Entry>, String> {
        let reader = self
            .sftp
            .read_dir(dir.to_string())
            .await
            .map_err(|error| error.to_string())?;

        let mut entries = Vec::new();
        for item in reader {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            if !hidden && name.starts_with('.') {
                continue;
            }
            let meta = item.metadata();
            entries.push(Entry {
                path: join_remote(dir, &name),
                kind: if meta.is_dir() {
                    "directory"
                } else if meta.is_symlink() {
                    "symlink"
                } else {
                    "file"
                },
                name,
                size: meta.size.unwrap_or(0),
                // SFTP reports whole seconds; the page wants milliseconds.
                modified_at: meta.mtime.unwrap_or(0) as i64 * 1000,
            });
        }
        order(&mut entries);
        Ok(entries)
    }

    async fn stat(&self, path: &str) -> Result<Stat, String> {
        let meta = self
            .sftp
            .metadata(path.to_string())
            .await
            .map_err(|error| error.to_string())?;
        Ok(Stat {
            size: meta.size.unwrap_or(0),
            modified: meta.mtime.unwrap_or(0) as u64,
        })
    }

    /*
     * The whole file, truncated and rewritten in place.
     *
     * Not atomic, and it cannot be. SFTP v3's `rename` fails when the target
     * exists, so the local trick -- write beside it, rename over it -- would
     * have to become "write, delete the original, rename", which opens a window
     * where the file does not exist at all. A crash there loses the file
     * outright; a crash here truncates it, which is worse than nothing happening
     * and better than nothing being there. OpenSSH's overwriting rename is an
     * extension that not every server has and that russh-sftp does not expose.
     *
     * This is what every editor that writes over SFTP does, for the same reason.
     */
    async fn write_bytes(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        use russh_sftp::protocol::OpenFlags;
        use tokio::io::AsyncWriteExt;

        let mut file = self
            .sftp
            .open_with_flags(
                path.to_string(),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|error| error.to_string())?;
        file.write_all(bytes)
            .await
            .map_err(|error| error.to_string())?;
        // Explicit, not left to the drop: a drop cannot report a failure, and a
        // short write nobody noticed is a truncated file reported as saved.
        file.flush().await.map_err(|error| error.to_string())?;
        Ok(())
    }

    /*
     * As the local one, but the loop matters more: an SFTP read is answered a
     * packet at a time, and asking for 512KB reliably comes back in pieces. A
     * single read would silently preview the first few kilobytes of every file.
     */
    async fn read_bytes(&self, path: &str, offset: u64, max: usize) -> Result<Vec<u8>, String> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt};

        let mut file = self
            .sftp
            .open(path.to_string())
            .await
            .map_err(|error| error.to_string())?;
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|error| error.to_string())?;

        let mut buffer = vec![0u8; max];
        let mut filled = 0;
        while filled < max {
            match file.read(&mut buffer[filled..]).await {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) => return Err(error.to_string()),
            }
        }
        buffer.truncate(filled);
        Ok(buffer)
    }

    /// The login directory, resolved. `.` is what the server would otherwise
    /// report, and a pane showing `.` cannot say where it is or go up from it.
    async fn default_path(&self) -> String {
        self.sftp
            .canonicalize(".".to_string())
            .await
            .unwrap_or_else(|_| "/".to_string())
    }

    async fn mkdir(&self, path: &str) -> Result<(), String> {
        self.sftp
            .create_dir(path.to_string())
            .await
            .map_err(|error| error.to_string())
    }

    async fn remove(&self, path: &str) -> Result<(), String> {
        let meta = self
            .sftp
            .symlink_metadata(path.to_string())
            .await
            .map_err(|error| error.to_string())?;
        if !meta.is_dir() {
            return self
                .sftp
                .remove_file(path.to_string())
                .await
                .map_err(|error| error.to_string());
        }

        // SFTP has no recursive remove; the tree has to be walked. Depth first,
        // because a directory cannot go until it is empty.
        let children = self
            .sftp
            .read_dir(path.to_string())
            .await
            .map_err(|error| error.to_string())?;
        for item in children {
            let name = item.file_name();
            if name == "." || name == ".." {
                continue;
            }
            Box::pin(self.remove(&join_remote(path, &name))).await?;
        }
        self.sftp
            .remove_dir(path.to_string())
            .await
            .map_err(|error| error.to_string())
    }

    async fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        self.sftp
            .rename(from.to_string(), to.to_string())
            .await
            .map_err(|error| error.to_string())
    }
}

// ------------------------------------------------------------------ paths ---

pub fn join_remote(base: &str, name: &str) -> String {
    if base.is_empty() || base == "/" {
        format!("/{name}")
    } else {
        format!("{}/{name}", base.trim_end_matches('/'))
    }
}

pub fn parent_remote(path: &str) -> String {
    let clean = path.trim_end_matches('/');
    match clean.rfind('/') {
        Some(0) | None => "/".to_string(),
        Some(cut) => clean[..cut].to_string(),
    }
}

pub fn join_local(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_string()
    } else {
        Path::new(base).join(name).to_string_lossy().into_owned()
    }
}

/// The parent, or `""` for the drive list -- which is where going up from a
/// drive root lands, and the only place an empty local path comes from.
pub fn parent_local(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    let clean = path.trim_end_matches(['/', '\\']);
    if clean.len() == 2 && clean.ends_with(':') {
        return String::new();
    }
    Path::new(clean)
        .parent()
        .map(|parent| parent.to_string_lossy().into_owned())
        .filter(|parent| parent != clean)
        .unwrap_or_default()
}

pub fn default_local() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .to_string_lossy()
        .into_owned()
}

// ----------------------------------------------------------------- panes ---

/// One transfer tab: its remote connection, and what each side is showing.
#[derive(Default)]
pub struct Pane {
    remote: Option<Arc<Remote>>,
    /// Set when a dead session was thrown away, cleared once the next listing
    /// has carried the news to the page. Kept here rather than returned from
    /// `remote` so that the dozen callers who do not care keep their signature.
    lost: bool,
    pub local_hidden: bool,
    pub remote_hidden: bool,
}

#[derive(Default)]
pub struct Transfers {
    panes: Mutex<HashMap<String, Pane>>,
    /// The transfer running in each tab, if any. One at a time per tab, which
    /// is what makes "a transfer is already running" a thing that can be said.
    jobs: Mutex<HashMap<String, Job>>,
}

impl Transfers {
    /// The remote for a pane, connecting on the first call.
    ///
    /// Deliberately lazy. Opening a transfer tab should not cost a second SSH
    /// handshake until something is actually asked of the remote side.
    pub async fn remote(&self, pane: &str, target: &ssh::Target) -> Result<Arc<Remote>, String> {
        {
            /*
             * A cached session that has since died is worse than no session at
             * all: every later listing fails against it and the pane never
             * recovers, because nothing ever replaces it. So the cache is
             * checked for life, not merely for presence.
             */
            let mut panes = self.panes.lock().unwrap();
            if let Some(entry) = panes.get_mut(pane) {
                match &entry.remote {
                    Some(existing) if !existing.is_closed() => return Ok(Arc::clone(existing)),
                    Some(_) => {
                        entry.remote = None;
                        entry.lost = true;
                    }
                    None => {}
                }
            }
        }

        let opened = Arc::new(Remote::open(target).await?);
        let mut panes = self.panes.lock().unwrap();
        let entry = panes.entry(pane.to_string()).or_default();
        // Another call may have won the race while this one was connecting.
        // Whichever landed first is the one everybody uses; the loser's session
        // is dropped here and its connection closes with it.
        Ok(entry.remote.get_or_insert(opened).clone())
    }

    /// Whether this pane's session has died since anyone last looked.
    ///
    /// Polled by the page every few seconds, so that a connection dropping is
    /// noticed while the user is looking at it rather than the next time they
    /// happen to click something.
    ///
    /// Reports once without needing to remember that it did: taking the session
    /// away is what makes the next poll find nothing to report.
    pub fn remote_died(&self, pane: &str) -> bool {
        let mut panes = self.panes.lock().unwrap();
        let Some(entry) = panes.get_mut(pane) else {
            return false;
        };
        match &entry.remote {
            Some(session) if session.is_closed() => {
                entry.remote = None;
                /*
                 * `lost` is not "the drop has been announced" -- it is "the
                 * reconnection still has to be". Two different events, and
                 * clearing it here, as this once did, meant the watcher
                 * announcing the drop also swallowed the "connected" line that
                 * should have followed the recovery.
                 */
                entry.lost = true;
                true
            }
            _ => false,
        }
    }

    /// Whether a connection was lost since this was last asked, and forget it.
    fn take_lost(&self, pane: &str) -> bool {
        let mut panes = self.panes.lock().unwrap();
        panes
            .get_mut(pane)
            .map(|entry| std::mem::take(&mut entry.lost))
            .unwrap_or(false)
    }

    pub fn hidden(&self, pane: &str, side: &str) -> bool {
        let panes = self.panes.lock().unwrap();
        panes.get(pane).is_some_and(|entry| {
            if side == "local" {
                entry.local_hidden
            } else {
                entry.remote_hidden
            }
        })
    }

    pub fn set_hidden(&self, pane: &str, side: &str, value: bool) {
        let mut panes = self.panes.lock().unwrap();
        let entry = panes.entry(pane.to_string()).or_default();
        if side == "local" {
            entry.local_hidden = value;
        } else {
            entry.remote_hidden = value;
        }
    }

    /// Forget a tab, closing its connection with it.
    pub fn close(&self, pane: &str) {
        self.cancel(pane);
        self.panes.lock().unwrap().remove(pane);
    }
}

// ------------------------------------------------------------ operations ---

/// What a pane is showing after a navigation.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listing {
    pub path: String,
    pub entries: Vec<Entry>,
    pub can_go_up: bool,
    /// The previous session had dropped and this listing opened a new one. The
    /// page says so in its log, because a transfer that silently reconnects is
    /// a transfer whose failures look like nothing happened.
    pub reconnected: bool,
}

pub async fn list(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    requested: &str,
) -> Result<Listing, String> {
    let hidden = transfers.hidden(pane, side);

    if side == "local" {
        let path = requested.trim().to_string();
        let entries = Local.list(&path, hidden)?;
        return Ok(Listing {
            can_go_up: !path.is_empty(),
            path,
            entries,
            reconnected: false,
        });
    }

    let remote = transfers.remote(pane, target).await?;
    // An empty request means "wherever the login lands", which is only knowable
    // once connected -- hence resolving it here rather than in the front end.
    let path = match requested.trim() {
        "" => remote.default_path().await,
        given => given.to_string(),
    };
    let entries = remote.list(&path, hidden).await?;
    Ok(Listing {
        can_go_up: path != "/",
        path,
        entries,
        // Read after the work, so a reconnect that happened part way through is
        // still reported rather than lost with the failed first attempt.
        reconnected: transfers.take_lost(pane),
    })
}

pub async fn make_dir(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
) -> Result<(), String> {
    if side == "local" {
        Local.mkdir(path)
    } else {
        transfers.remote(pane, target).await?.mkdir(path).await
    }
}

pub async fn remove(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    paths: &[String],
) -> Result<(), String> {
    for path in paths {
        if side == "local" {
            Local.remove(path)?;
        } else {
            transfers.remote(pane, target).await?.remove(path).await?;
        }
    }
    Ok(())
}

pub async fn stat(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
) -> Result<Stat, String> {
    if side == "local" {
        Local.stat(path)
    } else {
        transfers.remote(pane, target).await?.stat(path).await
    }
}

pub async fn read_bytes(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
    offset: u64,
    max: usize,
) -> Result<Vec<u8>, String> {
    if side == "local" {
        Local.read_bytes(path, offset, max)
    } else {
        transfers
            .remote(pane, target)
            .await?
            .read_bytes(path, offset, max)
            .await
    }
}

pub async fn write_bytes(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
    bytes: &[u8],
) -> Result<(), String> {
    if side == "local" {
        Local.write_bytes(path, bytes)
    } else {
        transfers
            .remote(pane, target)
            .await?
            .write_bytes(path, bytes)
            .await
    }
}

pub async fn rename(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    from: &str,
    to: &str,
) -> Result<(), String> {
    if side == "local" {
        Local.rename(from, to)
    } else {
        transfers.remote(pane, target).await?.rename(from, to).await
    }
}

// ------------------------------------------------------------------- jobs ---

/*
 * A transfer, from the moment the user drops a selection to the summary line.
 *
 * Three things make it more than a copy loop, and all three are why this lives
 * in Rust rather than in the page:
 *
 *   * A tree has to be walked before anything moves, because the progress bar is
 *     a promise and it cannot be made until the total is known.
 *   * Every file that already exists on the far side is a question, and the
 *     answer comes back from a dialog -- so the walk has to be able to stop and
 *     wait in the middle without blocking anything else in the window.
 *   * Cancelling has to be noticed inside the copy of a large file, not only
 *     between files.
 */

/// What the user answered about a file that was already there.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Overwrite,
    Skip,
    OverwriteAll,
    SkipAll,
    CancelAll,
}

impl Choice {
    fn parse(value: &str) -> Choice {
        match value {
            "overwrite" => Choice::Overwrite,
            "overwriteAll" => Choice::OverwriteAll,
            "skipAll" => Choice::SkipAll,
            "cancelAll" => Choice::CancelAll,
            _ => Choice::Skip,
        }
    }
}

/// A standing answer, once the user has said "all".
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Ask,
    Overwrite,
    Skip,
}

pub struct Job {
    cancel: Arc<AtomicBool>,
    answers: tokio::sync::mpsc::UnboundedSender<(u64, Choice)>,
}

struct File {
    source: String,
    target: String,
    name: String,
    size: u64,
}

#[derive(Default)]
struct Plan {
    dirs: Vec<String>,
    files: Vec<File>,
    total_bytes: u64,
}

/// How often the page is told where things have got to.
///
/// Not every block: a fast local copy would spend more time posting progress
/// than moving bytes, and no eye can read sixty updates a second anyway.
const TICK: Duration = Duration::from_millis(100);
/// The unit of copying. Large enough that SFTP's round trips are amortised,
/// small enough that a cancel is noticed promptly inside a big file.
const BLOCK: usize = 64 * 1024;

fn base_name(path: &str) -> String {
    path.trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .to_string()
}

impl Transfers {
    pub fn cancel(&self, pane: &str) {
        if let Some(job) = self.jobs.lock().unwrap().get(pane) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn answer(&self, pane: &str, id: u64, choice: &str) {
        if let Some(job) = self.jobs.lock().unwrap().get(pane) {
            let _ = job.answers.send((id, Choice::parse(choice)));
        }
    }

    pub fn busy(&self, pane: &str) -> bool {
        self.jobs.lock().unwrap().contains_key(pane)
    }
}

/// One side's view, so the walk and the copy do not each need the match.
struct Ends<'a> {
    transfers: &'a Transfers,
    pane: &'a str,
    target: &'a ssh::Target,
    from: &'a str,
    to: &'a str,
}

impl Ends<'_> {
    async fn list(&self, side: &str, dir: &str) -> Result<Vec<Entry>, String> {
        // Hidden entries are part of a tree even when the pane is not showing
        // them: copying a directory and silently leaving out its dotfiles is
        // how a deployment arrives without its configuration.
        if side == "local" {
            Local.list(dir, true)
        } else {
            self.transfers
                .remote(self.pane, self.target)
                .await?
                .list(dir, true)
                .await
        }
    }

    async fn size_of(&self, side: &str, path: &str) -> u64 {
        stat(self.transfers, self.pane, side, self.target, path)
            .await
            .map(|found| found.size)
            .unwrap_or(0)
    }

    async fn exists(&self, side: &str, path: &str) -> bool {
        stat(self.transfers, self.pane, side, self.target, path)
            .await
            .is_ok()
    }

    fn join(&self, side: &str, base: &str, name: &str) -> String {
        if side == "local" {
            join_local(base, name)
        } else {
            join_remote(base, name)
        }
    }
}

/// Walk the selection into a flat plan. Directories first in `dirs`, so that
/// creating them in order is enough to make every file's parent exist.
async fn scan(ends: &Ends<'_>, roots: &[(String, bool)], target_dir: &str) -> Result<Plan, String> {
    let mut plan = Plan::default();
    let mut queue: Vec<(String, String, bool)> = roots
        .iter()
        .map(|(path, is_dir)| {
            let name = base_name(path);
            (path.clone(), ends.join(ends.to, target_dir, &name), *is_dir)
        })
        .collect();

    while let Some((source, target, is_dir)) = queue.pop() {
        if !is_dir {
            let size = ends.size_of(ends.from, &source).await;
            plan.total_bytes += size;
            plan.files.push(File {
                name: base_name(&source),
                source,
                target,
                size,
            });
            continue;
        }

        plan.dirs.push(target.clone());
        for entry in ends.list(ends.from, &source).await? {
            // A symlink is followed only as far as what it points at looks like;
            // copying the link itself is not something SFTP offers portably.
            queue.push((
                entry.path,
                ends.join(ends.to, &target, &entry.name),
                entry.kind == "directory",
            ));
        }
    }

    // Shallowest first, so a parent is never created after its child.
    plan.dirs
        .sort_by_key(|path| path.matches(['/', '\\']).count());
    Ok(plan)
}

/// Copy one file across, block by block, reporting as it goes.
async fn copy_file(
    ends: &Ends<'_>,
    file: &File,
    cancel: &AtomicBool,
    seen: &mut u64,
    report: &mut impl FnMut(&File, u64, u64),
) -> Result<(), String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let remote = ends.transfers.remote(ends.pane, ends.target).await;
    let mut moved = 0u64;
    let mut last = Instant::now();

    /*
     * Four combinations reduce to two: a transfer always crosses sides, so one
     * end is the disk and the other is SFTP. Which is which is the only thing
     * that differs, and it is settled once here rather than per block.
     */
    if ends.from == "local" {
        let remote = remote?;
        let mut source = std::fs::File::open(&file.source).map_err(|e| e.to_string())?;
        let mut sink = remote
            .sftp
            .create(file.target.clone())
            .await
            .map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; BLOCK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(String::new());
            }
            let read = std::io::Read::read(&mut source, &mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            sink.write_all(&buffer[..read])
                .await
                .map_err(|e| e.to_string())?;
            moved += read as u64;
            *seen += read as u64;
            if last.elapsed() >= TICK {
                last = Instant::now();
                report(file, moved, *seen);
            }
        }
        sink.flush().await.map_err(|e| e.to_string())?;
    } else {
        let remote = remote?;
        let mut source = remote
            .sftp
            .open(file.source.clone())
            .await
            .map_err(|e| e.to_string())?;
        // Any parent the plan did not create -- a root file dropped straight
        // into a directory that is not there yet.
        if let Some(parent) = std::path::Path::new(&file.target).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut sink = std::fs::File::create(&file.target).map_err(|e| e.to_string())?;
        let mut buffer = vec![0u8; BLOCK];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(String::new());
            }
            let read = source.read(&mut buffer).await.map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            std::io::Write::write_all(&mut sink, &buffer[..read]).map_err(|e| e.to_string())?;
            moved += read as u64;
            *seen += read as u64;
            if last.elapsed() >= TICK {
                last = Instant::now();
                report(file, moved, *seen);
            }
        }
    }

    report(file, file.size.max(moved), *seen);
    Ok(())
}

/// What the page is told while a job runs. Raw bytes are not involved here --
/// these are a handful of small objects a second, so JSON is the right shape.
/// A line for the log pane. The key is the string table's; the detail is paths,
/// which no table can hold. The front end puts the two together.
fn note(out: &Channel<InvokeResponseBody>, key: &str, detail: String) {
    say(
        out,
        serde_json::json!({ "kind": "log", "key": key, "detail": detail }),
    );
}

fn say(out: &Channel<InvokeResponseBody>, value: serde_json::Value) {
    if let Ok(json) = serde_json::to_string(&value) {
        let _ = out.send(InvokeResponseBody::Json(json));
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub completed: u32,
    pub skipped: u32,
    pub failed: u32,
    pub cancelled: bool,
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    transfers: Arc<Transfers>,
    pane: String,
    from: String,
    to: String,
    target: ssh::Target,
    roots: Vec<(String, bool)>,
    target_dir: String,
    out: Channel<InvokeResponseBody>,
) -> Result<Summary, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let (answers, mut replies) = tokio::sync::mpsc::unbounded_channel();
    transfers.jobs.lock().unwrap().insert(
        pane.clone(),
        Job {
            cancel: Arc::clone(&cancel),
            answers,
        },
    );

    let ends = Ends {
        transfers: &transfers,
        pane: &pane,
        target: &target,
        from: &from,
        to: &to,
    };

    let outcome = drive(&ends, &roots, &target_dir, &cancel, &mut replies, &out).await;
    transfers.jobs.lock().unwrap().remove(&pane);
    say(&out, serde_json::json!({ "kind": "end" }));
    outcome
}

async fn drive(
    ends: &Ends<'_>,
    roots: &[(String, bool)],
    target_dir: &str,
    cancel: &AtomicBool,
    replies: &mut tokio::sync::mpsc::UnboundedReceiver<(u64, Choice)>,
    out: &Channel<InvokeResponseBody>,
) -> Result<Summary, String> {
    say(
        out,
        serde_json::json!({ "kind": "scanning", "totalFiles": 0, "totalBytes": 0 }),
    );

    let plan = scan(ends, roots, target_dir).await?;
    say(
        out,
        serde_json::json!({
            "kind": "scanning",
            "totalFiles": plan.files.len(),
            "totalBytes": plan.total_bytes,
        }),
    );

    for dir in &plan.dirs {
        if ends.to == "local" {
            let _ = Local.mkdir(dir);
        } else if let Ok(remote) = ends.transfers.remote(ends.pane, ends.target).await {
            // Already there is the outcome asked for, so a failure is ignored.
            let _ = remote.mkdir(dir).await;
        }
    }

    let mut summary = Summary {
        completed: 0,
        skipped: 0,
        failed: 0,
        cancelled: false,
    };
    let mut standing = Standing::Ask;
    let mut seen = 0u64;
    let mut question = 0u64;
    let total_files = plan.files.len();

    for (index, file) in plan.files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }

        if ends.exists(ends.to, &file.target).await {
            let answer = match standing {
                Standing::Overwrite => Choice::Overwrite,
                Standing::Skip => Choice::Skip,
                Standing::Ask => {
                    question += 1;
                    say(
                        out,
                        serde_json::json!({
                            "kind": "conflict",
                            "id": question,
                            "name": file.name,
                            "sourcePath": file.source,
                            "targetPath": file.target,
                            "sourceKind": "file",
                            "targetKind": "file",
                            "sourceSize": file.size,
                            "targetSize": ends.size_of(ends.to, &file.target).await,
                            "sourceModifiedAt": 0,
                            "targetModifiedAt": 0,
                            "remaining": total_files.saturating_sub(index + 1),
                        }),
                    );
                    // Answers for questions already withdrawn are discarded, so
                    // a stale click cannot decide the next file's fate.
                    loop {
                        match replies.recv().await {
                            Some((id, choice)) if id == question => break choice,
                            Some(_) => continue,
                            None => break Choice::CancelAll,
                        }
                    }
                }
            };

            match answer {
                Choice::Skip => {
                    summary.skipped += 1;
                    note(out, "itemSkipped", file.target.clone());
                    continue;
                }
                Choice::SkipAll => {
                    standing = Standing::Skip;
                    summary.skipped += 1;
                    note(out, "itemSkipped", file.target.clone());
                    continue;
                }
                Choice::OverwriteAll => standing = Standing::Overwrite,
                Choice::CancelAll => {
                    summary.cancelled = true;
                    break;
                }
                Choice::Overwrite => {}
            }
        }

        /*
         * The count of files finished *before* this one, which is what every
         * tick during it should say. It is deliberately a snapshot: `summary`
         * is borrowed by the loop and the closure below outlives the statement
         * that would update it.
         *
         * What used to be missing is the correction at the other end -- see the
         * `Ok` arm below.
         */
        let done_files = summary.completed;
        let opening = seen;
        let mut report = |file: &File, moved: u64, seen: u64| {
            say(
                out,
                serde_json::json!({
                    "kind": "progress",
                    "doneFiles": done_files,
                    "totalFiles": total_files,
                    "doneBytes": seen,
                    "totalBytes": plan.total_bytes,
                    "name": file.name,
                    "transferred": moved,
                    "total": file.size,
                }),
            );
        };
        report(file, 0, seen);
        note(
            out,
            if ends.from == "local" {
                "uploadingFile"
            } else {
                "downloadingFile"
            },
            format!("{} -> {}", file.source, file.target),
        );

        match copy_file(ends, file, cancel, &mut seen, &mut report).await {
            Ok(()) => {
                summary.completed += 1;
                /*
                 * A file landing is the one state change in this loop that
                 * produced no event of its own.
                 *
                 * `done_files` above is read before the copy starts, so every
                 * tick during a file reports the count as it was when the file
                 * began -- right for those ticks, and one behind from the
                 * instant the file lands. The correction used to arrive on the
                 * *next* file's opening tick, which hid the bug in the middle
                 * of a run and left it standing at the end: the last file never
                 * had a next tick, so a five file transfer finished reading
                 * `4/5`. With one file there is no next tick at all, and the
                 * card sat at `0/1` through a download that had completed,
                 * beside a byte count that had reached 100%.
                 *
                 * The byte figures were always right because `seen` is carried
                 * through `copy_file` by reference and advances with the
                 * blocks. Only the file count was a snapshot.
                 *
                 * `seen - opening` rather than `file.size`: it is what this
                 * copy actually moved, and the two differ if the file changed
                 * size between the scan and the copy. The bar should say what
                 * happened, not what was planned.
                 */
                say(
                    out,
                    serde_json::json!({
                        "kind": "progress",
                        "doneFiles": summary.completed,
                        "totalFiles": total_files,
                        "doneBytes": seen,
                        "totalBytes": plan.total_bytes,
                        "name": file.name,
                        "transferred": seen - opening,
                        "total": file.size,
                    }),
                );
            }
            // An empty reason is the cancel signal, not a failure with nothing
            // to say -- see `copy_file`, which is the only thing that sends one.
            Err(reason) if reason.is_empty() => {
                summary.cancelled = true;
                note(out, "transferCancelling", String::new());
                break;
            }
            Err(reason) => {
                summary.failed += 1;
                note(
                    out,
                    "itemFailed",
                    format!("{} -> {} ({reason})", file.source, file.target),
                );
            }
        }
    }

    Ok(summary)
}
