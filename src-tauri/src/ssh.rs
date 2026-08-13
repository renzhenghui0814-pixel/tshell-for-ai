//! The SSH session behind a terminal tab.
//!
//! # Why the output does not travel as JSON
//!
//! A shell that is printing produces small writes, thousands of them a second,
//! and Tauri's default IPC turns every command result into a JSON string. Pushing
//! terminal bytes that way spends more time escaping and parsing than the shell
//! spent producing them, and it shows up as the whole window stuttering while a
//! build scrolls past.
//!
//! So each session gets one [`Channel`], the bytes go through it as [`Raw`], and
//! this file gathers them first: nothing is pushed until the stream has been
//! quiet for [`FLUSH_WINDOW`] or [`FLUSH_BYTES`] have piled up. Sixty pushes a
//! second of a kilobyte each, rather than a thousand pushes of forty bytes. This
//! is an architectural constraint and not a tuning knob -- without it the design
//! does not work at all.
//!
//! [`Raw`]: InvokeResponseBody::Raw
//!
//! # Host keys are not checked
//!
//! Deliberately, and it is worth being plain about what that costs. The
//! connection is encrypted, but nothing here establishes *who* it is encrypted
//! to: anyone able to answer on the address can present their own key, relay to
//! the real server, and read the password and every keystroke in between. Both
//! `known_hosts` files on the machine are left alone -- neither read nor
//! written -- so nothing tshell does here affects `ssh` on the command line.
//!
//! This matches what the extension did (`ssh2` accepts any key unless given a
//! `hostVerifier`, and it was never given one). If it is ever revisited, the
//! shape that fits this codebase is a check in [`Client::check_server_key`] plus
//! a prompt through the shell's existing `confirm()` dialog, so that a changed
//! key is answerable from inside the window rather than by hand-editing a file.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use encoding_rs::Encoding;
use russh::client;
use russh::keys::ssh_key;
use russh::{ChannelMsg, Disconnect};
use serde::Serialize;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;
use tokio::time::Instant;

/// How long output may gather before it is pushed, however little there is.
const FLUSH_WINDOW: Duration = Duration::from_millis(8);
/// How much may gather within that window before it is pushed anyway.
const FLUSH_BYTES: usize = 32 * 1024;

/// The terminal type the remote shell is told it is talking to. What the
/// extension asked for, and what xterm.js actually implements.
const TERM: &str = "xterm-256color";

#[derive(Clone)]
pub enum Credential {
    Password(String),
    Key {
        path: PathBuf,
        passphrase: Option<String>,
    },
}

/// Cloned rather than rebuilt when the assistant opens its own SFTP channel:
/// the credentials were resolved once, for the terminal, and asking the keychain
/// again would be asking the same question twice.
#[derive(Clone)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub encoding: &'static Encoding,
    pub credential: Credential,
}

/// What the shell is told about a session, beside its bytes.
///
/// Sent down the same channel as the output, as JSON rather than raw, which is
/// what tells the two apart on the far side. Every one of these carries a key
/// from the string table rather than a sentence: the words belong to whichever
/// language the window is in, and that is the front end's business.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Status {
    /// The remote shell is open and about to speak.
    ///
    /// Pushed down the channel rather than inferred from this command returning,
    /// and pushed before the pump starts, because the front end clears the
    /// screen on it. The channel is ordered, so this is guaranteed to arrive
    /// ahead of the first byte of output -- whereas the command's own reply
    /// races it, and losing that race would wipe the login banner.
    Opened,
    /// The shell ended, or the connection dropped under it.
    Closed,
}

enum Command {
    Input(Vec<u8>),
    Resize { cols: u32, rows: u32 },
    /// Read the stream as something else from here on.
    Encoding(&'static Encoding),
}

/// The live sessions, one per terminal tab, keyed by the tab's pane id.
#[derive(Default)]
pub struct Sessions {
    live: Mutex<HashMap<String, mpsc::UnboundedSender<Command>>>,
    /// The assistant borrowing each terminal, where one is.
    ///
    /// Kept beside the sessions rather than inside them because the two have
    /// different lifetimes: a chat panel is opened and closed over a terminal
    /// that outlives it, and the terminal must not need reconnecting either way.
    agents: Mutex<HashMap<String, Arc<crate::ai::shell::AgentShell>>>,
}

impl Sessions {
    pub fn input(&self, pane: &str, encoding: &'static Encoding, text: &str) -> bool {
        let (bytes, _, _) = encoding.encode(text);
        self.send(pane, Command::Input(bytes.into_owned()))
    }

    /// Puts an assistant in front of one terminal's output.
    ///
    /// From here every chunk goes through it on the way to the tab, which is what
    /// lets a command's own output be told from the prompt around it. See
    /// `ai/shell.rs`.
    pub fn attach(&self, pane: &str, agent: Arc<crate::ai::shell::AgentShell>) {
        self.agents.lock().unwrap().insert(pane.to_string(), agent);
    }

    pub fn detach(&self, pane: &str) {
        self.agents.lock().unwrap().remove(pane);
    }

    pub fn agent(&self, pane: &str) -> Option<Arc<crate::ai::shell::AgentShell>> {
        self.agents.lock().unwrap().get(pane).cloned()
    }

    /// Whether this terminal still has a live connection behind it.
    ///
    /// What the chat panel asks before it offers to run anything: a tab whose
    /// shell has gone is a tab the assistant cannot type into.
    pub fn is_live(&self, pane: &str) -> bool {
        self.live.lock().unwrap().contains_key(pane)
    }

    pub fn resize(&self, pane: &str, cols: u32, rows: u32) -> bool {
        self.send(pane, Command::Resize { cols, rows })
    }

    /// Change how a live session's output is read, without reconnecting.
    ///
    /// The input side needs nothing: every keystroke is encoded against the
    /// config as it is at that moment. Output is the half that remembers,
    /// because a decoder holds state between chunks -- so it has to be told.
    pub fn set_encoding(&self, pane: &str, encoding: &'static Encoding) -> bool {
        self.send(pane, Command::Encoding(encoding))
    }

    /// Drop a session, closing its connection. Called when a tab closes.
    pub fn close(&self, pane: &str) {
        self.live.lock().unwrap().remove(pane);
        self.agents.lock().unwrap().remove(pane);
    }

    fn send(&self, pane: &str, command: Command) -> bool {
        let live = self.live.lock().unwrap();
        // A closed receiver means the pump has already stopped; the entry is
        // left for `close` or the next `open` to clear, because removing it
        // from here would mean taking a write lock on every keystroke.
        live.get(pane).is_some_and(|tx| tx.send(command).is_ok())
    }
}

/// The handler russh requires. It accepts every host key it is shown.
///
/// See the note at the top of this file: this is the extension's behaviour kept
/// as it was, and it is the reason the connection is confidential but not
/// authenticated.
pub struct Client;

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, _key: &ssh_key::PublicKey) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

/// Connect, start a shell, and leave a pump running that owns both ends.
///
/// Returns once the shell is open. Everything after that reaches the front end
/// through `out`.
pub async fn open(
    sessions: Arc<Sessions>,
    pane: String,
    target: Target,
    cols: u32,
    rows: u32,
    out: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let handle = connect(&target).await?;

    let channel = handle
        .channel_open_session()
        .await
        .map_err(|error| error.to_string())?;
    channel
        .request_pty(true, TERM, cols, rows, 0, 0, &[])
        .await
        .map_err(|error| error.to_string())?;
    channel
        .request_shell(true)
        .await
        .map_err(|error| error.to_string())?;

    // Before the pump, so that nothing the shell prints can precede it.
    say(&out, Status::Opened);

    let (tx, rx) = mpsc::unbounded_channel();
    sessions.live.lock().unwrap().insert(pane.clone(), tx);

    let sessions_for_pump = Arc::clone(&sessions);
    tauri::async_runtime::spawn(async move {
        pump(channel, rx, target.encoding, out.clone(), &sessions_for_pump, &pane).await;
        // The connection is gone; the tab is not. Clearing the entry is what
        // makes the next Enter reconnect rather than write into nothing.
        sessions_for_pump.close(&pane);
        say(&out, Status::Closed);
        let _ = handle
            .disconnect(Disconnect::ByApplication, "", "en")
            .await;
    });

    Ok(())
}

/// Connect and authenticate, and hand back the live session.
///
/// Shared with the transfer page, which gets its own connection rather than
/// borrowing the terminal's: SFTP is a subsystem on its own channel, and a
/// separate connection means a directory listing cannot stall behind a shell
/// that is busy, and closing either one leaves the other alone.
pub async fn connect(target: &Target) -> Result<client::Handle<Client>, String> {
    let config = Arc::new(client::Config {
        /*
         * How long a dead connection goes unnoticed: the interval times the
         * number of misses tolerated, so five seconds and two misses is about
         * fifteen. The extension's fifteen and three was three quarters of a
         * minute of a tab that looked fine and was not.
         *
         * The floor is not zero. Every interval is a packet on every idle
         * session, and a laptop that suspends for a moment should come back to
         * its terminals rather than to a row of dead ones.
         */
        keepalive_interval: Some(Duration::from_secs(5)),
        keepalive_max: 2,
        ..client::Config::default()
    });

    let mut handle = client::connect(config, (target.host.as_str(), target.port), Client)
        .await
        .map_err(|error| error.to_string())?;
    authenticate(&mut handle, target).await?;
    Ok(handle)
}

async fn authenticate(
    handle: &mut client::Handle<Client>,
    target: &Target,
) -> Result<(), String> {
    let accepted = match &target.credential {
        Credential::Password(password) => handle
            .authenticate_password(&target.username, password)
            .await
            .map_err(|error| error.to_string())?,
        Credential::Key { path, passphrase } => {
            let key = russh::keys::load_secret_key(path, passphrase.as_deref())
                .map_err(|error| format!("{}: {error}", path.display()))?;
            let hash = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|error| error.to_string())?
                .flatten();
            handle
                .authenticate_publickey(
                    &target.username,
                    russh::keys::PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                )
                .await
                .map_err(|error| error.to_string())?
        }
    };

    if accepted.success() {
        Ok(())
    } else {
        Err("authFailed".to_string())
    }
}

/// Own both ends of the channel until the tab or the far side lets go.
///
/// One loop, not two. An earlier version put the write side in its own task, and
/// closing a tab dropped the sender, which stopped that task -- and nothing
/// else. The read side went on waiting on a shell that was still running, the
/// pump never returned, `disconnect` was never reached, and the pty stayed
/// allocated on the server. Five closed tabs, five sessions still in `who`.
///
/// The three things that can happen therefore have to be waited on together:
/// output arriving, a command from the tab, and the gathered output going stale.
/// `Channel::split` is what makes that possible -- `read` and `write` are then
/// separate bindings, so a `select!` arm can borrow one without freezing the
/// other. All three futures are cancel-safe, which a `select!` over them
/// requires: each is a receive or a sleep, and dropping one loses nothing.
async fn pump(
    channel: russh::Channel<client::Msg>,
    mut rx: mpsc::UnboundedReceiver<Command>,
    encoding: &'static Encoding,
    out: Channel<InvokeResponseBody>,
    sessions: &Sessions,
    pane: &str,
) {
    let (mut read, write) = channel.split();

    let mut decoder = encoding.new_decoder();
    let mut pending: Vec<u8> = Vec::new();
    // Set when the buffer stops being empty, cleared when it is pushed. While it
    // is None there is nothing to wait for, so the loop blocks outright rather
    // than waking every 8ms to look at an empty buffer.
    let mut deadline: Option<Instant> = None;

    loop {
        // `pending()` never completes, which is how "no deadline" is expressed
        // as a future that `select!` can hold without it ever firing.
        let stale = async {
            match deadline {
                Some(at) => tokio::time::sleep_until(at).await,
                None => std::future::pending::<()>().await,
            }
        };

        tokio::select! {
            message = read.wait() => match message {
                Some(ChannelMsg::Data { data }) => pending.extend_from_slice(&data),
                // stderr and stdout share one stream here, exactly as they shared
                // one decoder under the extension: a terminal has one screen.
                Some(ChannelMsg::ExtendedData { data, .. }) => pending.extend_from_slice(&data),
                Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                // Window adjustments, exit statuses, request replies: real
                // messages, none of which put anything on the screen.
                Some(_) => continue,
            },

            command = rx.recv() => match command {
                Some(Command::Input(bytes)) => {
                    if write.data_bytes(bytes).await.is_err() {
                        break;
                    }
                }
                Some(Command::Resize { cols, rows }) => {
                    let _ = write.window_change(cols, rows, 0, 0).await;
                }
                Some(Command::Encoding(next)) => {
                    // Everything already gathered was written in the old
                    // encoding and has to be read in it. Whatever the old
                    // decoder is still holding -- half of a character split
                    // across the last chunk -- goes with it, which is the right
                    // answer: those bytes mean something different now.
                    flush(&mut decoder, &mut pending, &out, sessions, pane);
                    deadline = None;
                    decoder = next.new_decoder();
                }
                // The sender is gone, so the tab is gone. Say so on the wire
                // rather than just walking away: an EOF and a close is what
                // makes the far side reap the pty now instead of when the
                // socket eventually times out.
                None => {
                    let _ = write.eof().await;
                    let _ = write.close().await;
                    break;
                }
            },

            _ = stale => {
                flush(&mut decoder, &mut pending, &out, sessions, pane);
                deadline = None;
                continue;
            }
        }

        if pending.len() >= FLUSH_BYTES {
            flush(&mut decoder, &mut pending, &out, sessions, pane);
            deadline = None;
        } else if deadline.is_none() && !pending.is_empty() {
            deadline = Some(Instant::now() + FLUSH_WINDOW);
        }
    }

    flush(&mut decoder, &mut pending, &out, sessions, pane);
}

/// Decode what has gathered and push it as UTF-8 bytes.
///
/// The decoder is kept across calls on purpose. A multi-byte character can be
/// split across two reads from the wire -- it happens constantly with GB18030 at
/// a buffer boundary -- and a decoder that started fresh each time would turn
/// every one of those into a replacement character. Holding the state is what
/// makes the tail of one chunk join the head of the next.
fn flush(
    decoder: &mut encoding_rs::Decoder,
    pending: &mut Vec<u8>,
    out: &Channel<InvokeResponseBody>,
    sessions: &Sessions,
    pane: &str,
) {
    if pending.is_empty() {
        return;
    }

    let room = decoder
        .max_utf8_buffer_length(pending.len())
        .unwrap_or(pending.len() * 4);
    let mut text = String::with_capacity(room);
    // `last: false` -- the stream is not over, so an incomplete sequence at the
    // end is held back rather than reported as an error.
    // The buffer was sized from `max_utf8_buffer_length`, so one pass is always
    // enough and the result cannot be `OutputFull`.
    let (_, _, _) = decoder.decode_to_string(pending, &mut text, false);
    pending.clear();

    if text.is_empty() {
        return;
    }

    /*
     * The assistant sees every chunk and decides what the tab sees.
     *
     * It has to be every chunk, running command or not: the markers that delimit
     * a command's output can be split across two reads, and a shell that is
     * merely echoing is still the stream those markers arrive on. What comes back
     * is usually the whole of what went in -- while nothing is running it is
     * exactly that -- and is shorter only while a step is hiding its plumbing.
     */
    let shown = match sessions.agent(pane) {
        Some(agent) => agent.observe(&text),
        None => text,
    };
    if !shown.is_empty() {
        let _ = out.send(InvokeResponseBody::Raw(shown.into_bytes()));
    }
}

fn say(out: &Channel<InvokeResponseBody>, status: Status) {
    if let Ok(json) = serde_json::to_string(&status) {
        let _ = out.send(InvokeResponseBody::Json(json));
    }
}
