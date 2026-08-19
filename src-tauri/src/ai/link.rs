//! The assistant's own connection to the machine, for bytes that should not be
//! typed.
//!
//! # What this is for, and what it is not for
//!
//! Commands still go through the user's terminal. That is the product: the
//! assistant stands where the user stands, `cd` and `export` and `sudo` mean what
//! they mean, and every command it runs is one the user watched run. Nothing here
//! changes that, and nothing here runs commands.
//!
//! What it changes is the one thing the shared terminal was never good at.
//! Content cannot be typed -- see the note at the top of `files.rs` -- so it used
//! to travel as base64, split into chunks that each fit on a line, each chunk a
//! separate command with a full marker round trip through an interactive shell.
//! A 100 KB file was forty-odd round trips, and every one of them had to be
//! written into the shell, echoed back, delimited and stripped. Over SFTP it is
//! one write.
//!
//! # Why the commit still goes through the shell
//!
//! A write lands on a temporary file beside the target and is moved into place at
//! the end, so the target is the old file or the new one and never half of
//! either. SFTP cannot make that promise: version 3's `rename` fails outright if
//! the target exists, and the obvious repair -- remove, then rename -- opens a
//! window in which the file is simply gone. OpenSSH has an extension that does
//! the right thing; not every server does, and `russh-sftp` does not expose it.
//!
//! So the payload travels here and the commit stays a shell command. Forty-four
//! round trips become one, the `mv` is as atomic as it ever was, and the user
//! still sees a line in their terminal saying what was written -- which is worth
//! more than it sounds. A file that changed with nothing on screen is a file the
//! user has no memory of agreeing to.
//!
//! # Paths are absolute by the time they reach here
//!
//! This connection has its own session, so its idea of a working directory is the
//! login directory and not wherever the user has wandered to. A relative path
//! resolved here would land somewhere the user was not looking. `bridge.rs`
//! resolves against the terminal's own `pwd` before anything gets this far; there
//! is no fallback here, because a plausible wrong answer is worse than an error.

use std::sync::Arc;

use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::ssh;

/// Why an operation did not happen.
///
/// The distinction is the whole reason this is an enum. `Unavailable` says the
/// second connection could not be had at all -- no route to it, a server that
/// allows one session, an SFTP subsystem that is not enabled -- and the caller
/// should fall back to carrying the bytes through the shell, which is a real
/// configuration and not a broken one. `Failed` says the connection is fine and
/// the *file* is the problem, and asking that question again through the shell
/// would only get the same answer in different words.
#[derive(Debug, Clone)]
pub enum LinkError {
    Unavailable(String),
    Failed(String),
}

impl LinkError {
    pub fn message(&self) -> &str {
        match self {
            LinkError::Unavailable(why) | LinkError::Failed(why) => why,
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str(self.message())
    }
}

/// The live session, or nothing yet.
///
/// The handle is held beside the SFTP session rather than dropped: dropping it
/// closes the connection out from under the subsystem, which is the same reason
/// `transfer::Remote` keeps one.
struct Live {
    sftp: SftpSession,
    handle: russh::client::Handle<ssh::Client>,
}

/// The assistant's connection, opened when it is first needed.
///
/// Lazy because most tasks never touch a file: a session that asked three
/// questions and read no bytes should not have cost a second connection, a
/// second authentication, and a second entry in the server's logs.
///
/// Its own connection rather than the terminal's, for the reason the transfer
/// panel has its own: a subsystem opened on the terminal's connection would
/// queue behind whatever the shell is doing, and a file read while a build is
/// scrolling would wait for the build.
pub struct Link {
    target: ssh::Target,
    live: Mutex<Option<Live>>,
}

impl Link {
    pub fn new(target: ssh::Target) -> Arc<Self> {
        Arc::new(Link {
            target,
            live: Mutex::new(None),
        })
    }

    /// The bytes of one file, or why not.
    ///
    /// `limit` is checked against the size the server reports before anything is
    /// pulled, so a request for a file that turns out to be a database costs one
    /// round trip rather than a gigabyte of memory.
    pub async fn read(&self, path: &str, limit: usize) -> Result<Vec<u8>, LinkError> {
        let mut live = self.live.lock().await;
        let session = Self::ensure(&mut live, &self.target).await?;

        let meta = session
            .sftp
            .metadata(path.to_string())
            .await
            .map_err(|error| LinkError::Failed(error.to_string()))?;
        if let Some(size) = meta.size {
            if size as usize > limit {
                return Err(LinkError::Failed(format!(
                    "{path} is {size} bytes, which is more than may be read this way."
                )));
            }
        }

        session
            .sftp
            .read(path.to_string())
            .await
            .map_err(|error| LinkError::Failed(error.to_string()))
    }

    /// Puts bytes at a path, creating or truncating it.
    ///
    /// `create` rather than the session's own `write`, which opens with `WRITE`
    /// alone: that fails on a path with nothing at it -- which is every path this
    /// is called with, since it is always a fresh temporary -- and, on a path with
    /// something at it, would leave the tail of a longer previous file behind the
    /// new content.
    pub async fn write(&self, path: &str, bytes: &[u8]) -> Result<(), LinkError> {
        let mut live = self.live.lock().await;
        let session = Self::ensure(&mut live, &self.target).await?;

        let mut file = session
            .sftp
            .open_with_flags(
                path.to_string(),
                OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
            )
            .await
            .map_err(|error| LinkError::Failed(error.to_string()))?;
        file.write_all(bytes).await.map_err(|error| LinkError::Failed(error.to_string()))?;
        // Flushed explicitly rather than left to the drop: a drop cannot report a
        // failure, and a short write that nobody noticed becomes a truncated file
        // that the commit then moves into place over a good one.
        file.flush().await.map_err(|error| LinkError::Failed(error.to_string()))?;
        Ok(())
    }

    /// What is at a path: `Some(true)` for a directory, `Some(false)` for
    /// anything else, `None` for nothing at all.
    ///
    /// Every failure here is `Unavailable` rather than `Failed`, which is true in
    /// no other method and is deliberate. A `stat` can fail for reasons SFTP has
    /// no useful word for -- search permission missing on a parent directory, a
    /// server that answers FAILURE where it means NO_SUCH_FILE -- and the shell
    /// answers the same question while coping with all of them. Stepping aside
    /// beats reporting a refusal the shell would not have made.
    pub async fn probe(&self, path: &str) -> Result<Option<bool>, LinkError> {
        let mut live = self.live.lock().await;
        let session = Self::ensure(&mut live, &self.target).await?;
        match session.sftp.try_exists(path.to_string()).await {
            Ok(false) => Ok(None),
            Ok(true) => match session.sftp.metadata(path.to_string()).await {
                Ok(meta) => Ok(Some(meta.is_dir())),
                Err(error) => Err(LinkError::Unavailable(error.to_string())),
            },
            Err(error) => Err(LinkError::Unavailable(error.to_string())),
        }
    }

    /// Drops the connection. Called when the panel closes, so the server reaps
    /// the session now rather than when the socket eventually times out.
    pub async fn close(&self) {
        if let Some(live) = self.live.lock().await.take() {
            let _ = live
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
        }
    }

    /// The live session, opening or reopening one if there is not one already.
    ///
    /// Taken as `&mut Option<Live>` rather than through `&self` so the borrow
    /// checker can see that the caller still holds the lock: two tasks opening a
    /// second connection apiece because they both found `None` is exactly the
    /// thing the lock is for.
    async fn ensure<'a>(
        live: &'a mut Option<Live>,
        target: &ssh::Target,
    ) -> Result<&'a Live, LinkError> {
        // A handle that has closed under us -- the network dropped, the server
        // restarted -- is worth nothing, and a caller that got an error from it
        // would have no way to ask for a fresh one.
        if live.as_ref().is_some_and(|session| session.handle.is_closed()) {
            *live = None;
        }

        if live.is_none() {
            // Everything on the way up is `Unavailable`: none of it is about the
            // file the caller wanted, and all of it means the shell has to carry
            // the bytes instead.
            let handle = ssh::connect(target).await.map_err(LinkError::Unavailable)?;
            let channel = handle
                .channel_open_session()
                .await
                .map_err(|error| LinkError::Unavailable(error.to_string()))?;
            channel
                .request_subsystem(true, "sftp")
                .await
                .map_err(|error| LinkError::Unavailable(error.to_string()))?;
            let sftp = SftpSession::new(channel.into_stream())
                .await
                .map_err(|error| LinkError::Unavailable(error.to_string()))?;
            *live = Some(Live { sftp, handle });
        }

        Ok(live.as_ref().expect("just opened"))
    }
}
