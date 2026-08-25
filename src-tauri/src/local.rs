//! Local terminals: shells run on this machine through a pseudo-terminal.
//!
//! A local terminal is the other half of the product. An SSH session is a
//! shell on a machine at the far end of the network; a local one is a shell
//! on this machine -- a cmd or PowerShell console, a Visual Studio developer
//! prompt, anything the user can name. Both end in the same xterm pane,
//! because a terminal page never learns which kind it is showing: it receives
//! bytes and sends keystrokes either way.
//!
//! The pseudo-terminal is ConPTY on Windows and openpty elsewhere, through
//! `portable-pty`. Its transport is UTF-8 end to end. Local terminal sessions
//! deliberately have no selectable decoder: changing a decoder underneath a
//! live console also reinterprets terminal control traffic. A console has no
//! host key and no reconnect policy of its own: it is here until it exits, and
//! Enter on a dead one opens it again.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::sync::mpsc;

use crate::config::{Service, VsDevService};
use crate::ssh::Status;

enum Command {
    Input(Vec<u8>),
    Resize { cols: u32, rows: u32 },
}

/// The live local terminals, keyed by pane id -- the same shape as ssh's
/// [`crate::ssh::Sessions`], and for the same reason: a terminal tab is
/// identified by the pane it lives in, and the pump behind it answers
/// commands on a channel.
#[derive(Default)]
pub struct Sessions {
    live: Mutex<HashMap<String, mpsc::UnboundedSender<Command>>>,
}

impl Sessions {
    pub fn input(&self, pane: &str, text: &str) -> bool {
        self.send(pane, Command::Input(text.as_bytes().to_vec()))
    }

    pub fn resize(&self, pane: &str, cols: u32, rows: u32) -> bool {
        self.send(pane, Command::Resize { cols, rows })
    }

    /// Drop a session, closing its console. Called when a tab closes.
    pub fn close(&self, pane: &str) {
        self.live.lock().unwrap().remove(pane);
    }

    fn send(&self, pane: &str, command: Command) -> bool {
        let live = self.live.lock().unwrap();
        // A closed receiver means the pump has already stopped; the entry is
        // left for `close` or the next `open` to clear.
        live.get(pane).is_some_and(|tx| tx.send(command).is_ok())
    }
}

/// The process to start, resolved from a configured local terminal.
pub struct Plan {
    program: String,
    args: Vec<String>,
    cwd: String,
}

/// Turn a configured local terminal into a process to start.
///
/// `None` means the kind cannot run here -- a Visual Studio developer prompt
/// has no counterpart on a non-Windows machine -- or the entry is missing what
/// its kind requires. The config's `normalize` drops the second case, so in
/// practice `None` here is the first.
pub fn plan(service: &Service) -> Option<Plan> {
    if cfg!(windows) {
        match service {
            Service::Cmd(term) => Some(Plan {
                program: "cmd.exe".to_string(),
                args: cmd_arguments(),
                cwd: term.cwd.clone(),
            }),
            Service::PowerShell(term) => Some(Plan {
                program: "powershell.exe".to_string(),
                args: powershell_arguments(),
                cwd: term.cwd.clone(),
            }),
            Service::VsDev(term) => {
                let path = vs_script(term)?;
                // `cmd /K "call "<path>" <arch>"`: `/K` keeps the window open
                // after the batch has run, and `call` makes the batch's `set`
                // stick in the cmd that stays -- a bare invoke would run the
                // batch and then return to a cmd that lost all of it.
                let is_vsdevcmd = path.file_name().is_some_and(|name| {
                    name.to_string_lossy().eq_ignore_ascii_case("vsdevcmd.bat")
                });
                let command = if is_vsdevcmd {
                    format!(
                        "call \"{}\" -arch={}",
                        path.display(),
                        vsdevcmd_arch(&term.arch)
                    )
                } else {
                    format!("call \"{}\" {}", path.display(), vs_arch(&term.arch))
                };
                // `cmd /K` reparses quote characters in its command argument.
                // Put the call in a short, unique batch file instead: cmd sees
                // an unquoted path, while the batch parser handles the VS path
                // with spaces normally. The script deletes itself before the
                // interactive prompt is shown.
                let launcher = vs_launcher(&format!("chcp 65001 > nul\r\n{command}"))?;
                Some(Plan {
                    program: "cmd.exe".to_string(),
                    args: vec![
                        "/D".to_string(),
                        "/K".to_string(),
                        format!("call {}", launcher.display()),
                    ],
                    cwd: term.cwd.clone(),
                })
            }
            Service::Ssh(_) => None,
        }
    } else {
        // The two plain consoles are shells wherever this runs; a developer
        // prompt is a Windows thing and has no counterpart here.
        let shell = || std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        match service {
            Service::Cmd(term) | Service::PowerShell(term) => Some(Plan {
                program: shell(),
                args: Vec::new(),
                cwd: term.cwd.clone(),
            }),
            Service::VsDev(_) | Service::Ssh(_) => None,
        }
    }
}

fn cmd_arguments() -> Vec<String> {
    vec![
        "/D".to_string(),
        "/K".to_string(),
        "chcp 65001 > nul".to_string(),
    ]
}

fn powershell_arguments() -> Vec<String> {
    vec![
        "-NoLogo".to_string(),
        "-NoExit".to_string(),
        "-Command".to_string(),
        "chcp 65001 > $null; $e = [Text.UTF8Encoding]::new($false); [Console]::InputEncoding = $e; [Console]::OutputEncoding = $e; $OutputEncoding = $e".to_string(),
    ]
}

fn vs_launcher(command: &str) -> Option<PathBuf> {
    reclaim_old_vs_launchers();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let path = std::env::temp_dir().join(format!("tshell-vs-{}-{stamp}.cmd", std::process::id()));
    // The generated file is ASCII-only except for a user-supplied path, which
    // Windows cmd reads through its active code page; VS's standard path is
    // ASCII and custom non-ASCII paths are handled by the user's `cmd` setup.
    std::fs::write(&path, format!("@echo off\r\n{command}\r\n")).ok()?;
    Some(path)
}

fn reclaim_old_vs_launchers() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("tshell-vs-"))
            && entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age.as_secs() > 3600);
        if stale {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The Visual Studio 2013 developer prompt is a product feature, not a path
/// users should have to memorise. A custom script still wins (including newer
/// Visual Studio installations); otherwise find the standard VS 2013 layout.
fn vs_script(term: &VsDevService) -> Option<PathBuf> {
    if !term.path.trim().is_empty() {
        // Keep a user-supplied path verbatim.  If it has gone stale, cmd shows
        // its own useful diagnostic in the terminal rather than turning the
        // action into a generic and unhelpful "not runnable" error.
        // Paths copied from Explorer are often wrapped in quotes. They are
        // quoting syntax, not part of the path; retaining them would make the
        // `call` builder add a second pair and cmd would try to execute the
        // quote characters as part of the file name.
        return Some(PathBuf::from(term.path.trim().trim_matches('"')));
    }
    // The installer sets this variable to `...\\Common7\\Tools\\`.  It is
    // the most reliable signal for a side-by-side VS 2013 install, and avoids
    // guessing which of the two Program Files roots the user chose.
    if let Some(tools) = std::env::var_os("VS120COMNTOOLS") {
        let tools = PathBuf::from(tools);
        if let Some(root) = tools.parent().and_then(|common7| common7.parent()) {
            let path = root.join("VC").join("vcvarsall.bat");
            if path.is_file() {
                return Some(path);
            }
        }
    }

    let roots = [
        std::env::var_os("ProgramFiles(x86)"),
        std::env::var_os("ProgramFiles"),
    ];
    roots
        .into_iter()
        .flatten()
        .map(PathBuf::from)
        .find_map(|root| {
            let path = root
                .join("Microsoft Visual Studio 12.0")
                .join("VC")
                .join("vcvarsall.bat");
            path.is_file().then_some(path)
        })
}

/// `vcvarsall.bat` calls 64-bit tools `amd64`; the panel keeps the friendlier
/// x86/x64 spelling. `VsDevCmd.bat` accepts the same canonical architecture.
fn vs_arch(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "x64" | "amd64" => "amd64",
        _ => "x86",
    }
}

/// `VsDevCmd.bat` has a separate spelling from `vcvarsall.bat`: it accepts
/// the architecture the user sees in the UI, not vcvarsall's `amd64` token.
fn vsdevcmd_arch(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "x64" | "amd64" => "x64",
        _ => "x86",
    }
}

/// Start a local terminal and leave a pump running that owns both ends.
///
/// Returns once the console is open. Everything after that reaches the front
/// end through `out`, exactly as ssh::open does for a remote session.
pub fn open(
    sessions: Arc<Sessions>,
    pane: String,
    plan: Plan,
    cols: u32,
    rows: u32,
    out: Channel<InvokeResponseBody>,
) -> Result<(), String> {
    let spawn = spawn_console(&plan, cols.max(1), rows.max(1))?;

    // Before the pump, so that nothing the console prints can precede it --
    // the same guarantee ssh::open makes, for the same reason.
    say(&out, Status::Opened);

    let (tx, rx) = mpsc::unbounded_channel();
    sessions.live.lock().unwrap().insert(pane.clone(), tx);

    let sessions_for_pump = Arc::clone(&sessions);
    std::thread::spawn(move || {
        pump(spawn, rx, out, sessions_for_pump, pane);
    });

    Ok(())
}

/// A running console: the output stream to read, the input to write, and the
/// way to shut it down, behind one shape.
struct Console {
    output: Box<dyn Read + Send>,
    input: Box<dyn Write + Send>,
    resize: Option<Box<dyn Fn(u32, u32) + Send>>,
    shutdown: Box<dyn FnOnce() + Send>,
}

/// Start the program in a real pseudo-terminal.
///
/// `portable-pty` gives us ConPTY on Windows and openpty elsewhere, which is
/// what makes a full-screen interactive program (vim, an interactive REPL)
/// behave like it does in a real console. The pane renders the same either
/// way -- it only ever sees bytes -- so this is the one place the two worlds
/// meet.
fn spawn_console(plan: &Plan, cols: u32, rows: u32) -> Result<Console, String> {
    use portable_pty::{native_pty_system, CommandBuilder, PtySize};

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.max(1).min(u16::MAX as u32) as u16,
            cols: cols.max(1).min(u16::MAX as u32) as u16,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut command = CommandBuilder::new(&plan.program);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    for arg in &plan.args {
        command.arg(arg);
    }
    if !plan.cwd.trim().is_empty() {
        command.cwd(&plan.cwd);
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|e| e.to_string())?;
    // The slave side is the child's now; keeping ours would hold the pty open
    // after the child exits.
    drop(pair.slave);

    // The reader and the writer come out of the master; the master itself
    // stays for resize.
    let output = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let input = pair.master.take_writer().map_err(|e| e.to_string())?;

    let resize = Box::new(move |cols: u32, rows: u32| {
        let _ = pair.master.resize(PtySize {
            rows: rows.max(1).min(u16::MAX as u32) as u16,
            cols: cols.max(1).min(u16::MAX as u32) as u16,
            pixel_width: 0,
            pixel_height: 0,
        });
    });

    let shutdown = Box::new(move || {
        // On Windows, dropping the master also closes the console, which takes
        // every process attached to it with it; the kill is belt to that.
        let _ = child.kill();
    });

    Ok(Console {
        output,
        input,
        resize: Some(resize),
        shutdown,
    })
}

/// One thread owns the whole console: it answers commands from the tab --
/// keystrokes, resizes -- until either end goes away, and a second thread
/// reads the output side and forwards decoded bytes.
fn pump(
    console: Console,
    mut rx: mpsc::UnboundedReceiver<Command>,
    out: Channel<InvokeResponseBody>,
    sessions: Arc<Sessions>,
    pane: String,
) {
    let mut output = console.output;
    let mut input = console.input;
    let resize = console.resize;

    let read_out = out.clone();
    let read_sessions = Arc::clone(&sessions);
    let read_pane = pane.clone();
    let read_thread = std::thread::spawn(move || {
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut pending: Vec<u8> = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            match output.read(&mut buffer) {
                // EOF, or the console closing under us: it is gone.
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    pending.extend_from_slice(&buffer[..n]);
                    flush(&mut decoder, &mut pending, &read_out);
                }
            }
        }
        say(&read_out, Status::Closed);
        // Clearing the entry is what makes the next Enter reopen rather than
        // write into nothing -- and it drops the sender, which ends the
        // command loop below.
        read_sessions.close(&read_pane);
    });

    while let Some(command) = rx.blocking_recv() {
        match command {
            Command::Input(bytes) => {
                let _ = input.write_all(&bytes);
                let _ = input.flush();
            }
            Command::Resize { cols, rows } => {
                if let Some(resize) = &resize {
                    resize(cols.max(1), rows.max(1));
                }
            }
        }
    }

    // The tab is gone. Shut the console down; the read thread wakes on the
    // closed output and reports Closed itself, and is detached so nothing
    // waits on it.
    drop(input);
    (console.shutdown)();
    let _ = read_thread;
}

/// Decode what has gathered and push it as UTF-8 bytes, holding the decoder
/// across calls so a multi-byte character split across two reads stays whole.
fn flush(
    decoder: &mut encoding_rs::Decoder,
    pending: &mut Vec<u8>,
    out: &Channel<InvokeResponseBody>,
) {
    if pending.is_empty() {
        return;
    }
    let room = decoder
        .max_utf8_buffer_length(pending.len())
        .unwrap_or(pending.len() * 4);
    let mut text = String::with_capacity(room);
    let (_, _, _) = decoder.decode_to_string(pending, &mut text, false);
    pending.clear();
    if text.is_empty() {
        return;
    }
    // Windows `cls` uses CSI 2J. ConPTY may leave old lines in xterm's
    // scrollback/viewport boundary after that operation; CSI 3J is xterm's
    // explicit erase-scrollback companion and makes a local clear unambiguous.
    let text = text.replace("\x1b[2J", "\x1b[3J\x1b[2J");
    let _ = out.send(InvokeResponseBody::Raw(text.into_bytes()));
}

fn say(out: &Channel<InvokeResponseBody>, status: Status) {
    if let Ok(json) = serde_json::to_string(&status) {
        let _ = out.send(InvokeResponseBody::Json(json));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> Service {
        Service::Cmd(crate::config::ShellService {
            id: String::new(),
            name: String::new(),
            cwd: String::new(),
        })
    }

    fn powershell() -> Service {
        Service::PowerShell(crate::config::ShellService {
            id: String::new(),
            name: String::new(),
            cwd: String::new(),
        })
    }

    fn vs() -> Service {
        Service::VsDev(VsDevService {
            id: String::new(),
            name: String::new(),
            path: String::new(),
            arch: String::new(),
            cwd: String::new(),
        })
    }

    /// A developer prompt becomes `cmd /K "call "<path>" <arch>"` -- the
    /// shape that keeps the window open with the toolshed's environment.
    #[test]
    fn a_vs_dev_prompt_loads_the_toolshed_into_a_staying_cmd() {
        let mut t = vs();
        let Service::VsDev(value) = &mut t else {
            unreachable!()
        };
        value.path =
            "D:\\Program Files (x86)\\Microsoft Visual Studio 12.0\\VC\\vcvarsall.bat".into();
        value.arch = "x64".into();
        let plan = plan(&t).expect("runnable");
        assert_eq!(plan.program, "cmd.exe");
        assert_eq!(plan.args[..2], ["/D", "/K"]);
        assert!(plan.args[2].contains("tshell-vs-"));
    }

    /// No architecture means x86, the default the panel promises.
    #[test]
    fn a_vs_dev_prompt_without_an_architecture_defaults_to_x86() {
        let mut t = vs();
        let Service::VsDev(value) = &mut t else {
            unreachable!()
        };
        value.path = "C:\\vs\\vcvarsall.bat".into();
        let plan = plan(&t).expect("runnable");
        assert!(plan.args[2].contains("tshell-vs-"));
    }

    #[test]
    fn a_quoted_vs_script_path_is_normalized_before_cmd_quotes_it() {
        let mut t = vs();
        let Service::VsDev(value) = &mut t else {
            unreachable!()
        };
        value.path =
            "\"D:\\Program Files (x86)\\Microsoft Visual Studio 12.0\\VC\\vcvarsall.bat\"".into();
        let plan = plan(&t).expect("runnable");
        assert!(plan.args[2].contains("tshell-vs-"));
    }

    #[test]
    fn visual_studio_architectures_use_vcvarsall_names() {
        assert_eq!(vs_arch("x86"), "x86");
        assert_eq!(vs_arch("x64"), "amd64");
        assert_eq!(vs_arch("amd64"), "amd64");
    }

    #[test]
    fn vsdevcmd_uses_its_own_architecture_spelling() {
        assert_eq!(vsdevcmd_arch("x86"), "x86");
        assert_eq!(vsdevcmd_arch("amd64"), "x64");
        assert_eq!(vsdevcmd_arch("x64"), "x64");
    }

    /// The plain consoles name their shells and initialise UTF-8 before
    /// accepting interactive input.
    #[test]
    fn plain_consoles_resolve_to_their_shells() {
        let cmd = plan(&cmd()).expect("runnable");
        assert_eq!(cmd.program, "cmd.exe");
        assert_eq!(cmd.args, ["/D", "/K", "chcp 65001 > nul"]);
        let ps = plan(&powershell()).expect("runnable");
        assert_eq!(ps.program, "powershell.exe");
        assert_eq!(ps.args[..3], ["-NoLogo", "-NoExit", "-Command"]);
    }

    /// The whole local console path, end to end: spawn a real cmd, type a
    /// command into it, and read the answer back. This is what the pane does
    /// on every keystroke, so a regression here is a terminal that shows
    /// nothing.
    #[test]
    #[cfg(windows)]
    fn a_cmd_console_answers() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};

        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("openpty");
        let mut child = pair
            .slave
            .spawn_command(CommandBuilder::new("cmd.exe"))
            .expect("spawn cmd");
        drop(pair.slave);

        std::thread::sleep(std::time::Duration::from_millis(800));
        eprintln!("[test] child exited: {:?}", child.try_wait());

        let mut output = pair.master.try_clone_reader().expect("reader");
        let mut input = pair.master.take_writer().expect("writer");

        let _ = input.write_all(b"echo tshell-before-cls\r\ncls\r\necho tshell-after-cls\r\n");
        let _ = input.flush();

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            let mut total = String::new();
            let mut chunk = [0u8; 4096];
            loop {
                match output.read(&mut chunk) {
                    Ok(0) => {
                        eprintln!("[test] READ EOF");
                        break;
                    }
                    Err(e) => {
                        eprintln!("[test] READ ERR {e:?}");
                        break;
                    }
                    Ok(n) => {
                        let piece = String::from_utf8_lossy(&chunk[..n]);
                        eprintln!("[test] READ {:?}", piece);
                        total.push_str(&piece);
                        if total.contains("tshell-after-cls") {
                            break;
                        }
                    }
                }
            }
            let _ = tx.send(total);
        });
        let text = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_default();
        let _ = child.kill();

        assert!(
            text.contains("tshell-after-cls"),
            "the console should echo the command back; output was: {text:?}"
        );
        assert!(
            text.contains("\x1b[2J"),
            "cls must emit a clear-screen sequence; output was: {text:?}"
        );
    }
}
