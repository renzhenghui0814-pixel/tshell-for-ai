// The commands the shell calls, and the translation between what the config
// file holds and what the pages -- which are the extension's, unedited -- expect
// to be handed.
//
// Stage 1 put the server panel on the real config: it is redesigned in
// `config.rs`, its secrets live in `secrets.rs`, and nothing below invents data.
// Stage 2 added the terminal: `ssh.rs` owns the connections, and the four
// `terminal_*` commands are the whole of what the front end can do to one.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai;
mod atomic;
mod config;
mod fonts;
mod hosts;
mod i18n;
mod preview;
mod schemes;
mod secrets;
mod ssh;
mod theme;
mod transfer;

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::{Channel, InvokeResponseBody};
use tauri::Manager;
use tauri::State;

use config::{AppConfig, Auth, Encoding, Group, Language, Server};
use secrets::{server_passphrase, server_password, Backend, Store};

// ------------------------------------------------------------------ views ---

/*
 * What the page is handed, which is not what the file holds.
 *
 * `ui/servers/servers.js` is the extension's, unedited, and it reads
 * `server.authType` and `server.privateKeyPath` hanging off the side of the
 * server object. The file now stores that as one tagged union, which is the
 * shape that cannot express the combinations that were never valid.
 *
 * Rather than edit 261 lines of working front end to follow, the translation
 * happens here, at the boundary, in the one place both shapes are already in
 * scope. It is a dozen lines and it keeps the page contract exactly as it was.
 */
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerView {
    id: String,
    name: String,
    host: String,
    port: u16,
    username: String,
    encoding: Encoding,
    auth_type: &'static str,
    private_key_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GroupView {
    id: String,
    name: String,
    servers: Vec<ServerView>,
}

impl From<&Server> for ServerView {
    fn from(server: &Server) -> Self {
        let (auth_type, private_key_path) = match &server.auth {
            Auth::Password { .. } => ("password", String::new()),
            Auth::PrivateKey { path, .. } => ("privateKey", path.clone()),
        };
        ServerView {
            id: server.id.clone(),
            name: server.name.clone(),
            host: server.host.clone(),
            port: server.port,
            username: server.username.clone(),
            encoding: server.encoding,
            auth_type,
            private_key_path,
        }
    }
}

/*
 * A blank name means "the default group", in whatever language is set.
 *
 * The page used to decide this itself, by checking whether the group's id was
 * literally `default` -- which drew the translated label over the stored name
 * and so made the default group the one group that could not be renamed. The
 * rename was saved; it was just never shown. Keying off the name instead means
 * renaming it works, and leaves it following the language until someone does.
 */
fn group_view(group: &Group, language: Language) -> GroupView {
    GroupView {
        id: group.id.clone(),
        name: if group.name.trim().is_empty() {
            i18n::t(language, "defaultGroup")
        } else {
            group.name.clone()
        },
        servers: group.servers.iter().map(ServerView::from).collect(),
    }
}

/// Where this installation keeps things, for the settings page to state plainly.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Storage {
    backend: Backend,
    /// Why the keychain was not used, when it was not. `None` on a healthy system.
    reason: Option<String>,
    /// The fallback file. Only meaningful, and only shown, under `Backend::File`.
    secrets_path: String,
    config_path: String,
}

fn storage(store: &Store) -> Storage {
    Storage {
        backend: store.backend(),
        reason: store.reason().map(str::to_string),
        secrets_path: store.path().display().to_string(),
        config_path: config::config_path().display().to_string(),
    }
}

/// Everything the sidebar needs for one render.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PanelState {
    groups: Vec<GroupView>,
    /// Keyed by server id, as `servers.js` expects. Empty string means none saved.
    passwords: HashMap<String, String>,
    private_key_passphrases: HashMap<String, String>,
    ai_enabled: bool,
    language: Language,
    show_hidden_files: bool,
    storage: Storage,
}

/*
 * What the dialog sends back, which is also the old shape.
 *
 * Every field is optional at this boundary because it arrives from a web page:
 * `normalize` on the way in is what makes the rest of the code able to assume
 * anything at all.
 */
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ServerInput {
    id: String,
    name: String,
    host: String,
    port: Option<u16>,
    username: String,
    auth_type: String,
    private_key_path: String,
    encoding: String,
}

// ------------------------------------------------------------- plumbing ---

pub(crate) fn load() -> Result<AppConfig, String> {
    config::load().map_err(|error| error.message(&config::config_path()))
}

fn panel_state(config: &AppConfig, store: &Store) -> PanelState {
    let mut passwords = HashMap::new();
    let mut private_key_passphrases = HashMap::new();
    for group in &config.groups {
        for server in &group.servers {
            /*
             * The plaintext goes to the page because the edit dialog fills its
             * password field from it, exactly as it did under VS Code. Moving
             * the secrets to the keychain was about what sits on disk in a file
             * the user is invited to read -- not about the dialog that exists
             * to show and change them.
             */
            let password = match &server.auth {
                Auth::Password { has_password: true } => {
                    store.get(&server_password(&server.id)).unwrap_or_default()
                }
                _ => String::new(),
            };
            let passphrase = match &server.auth {
                Auth::PrivateKey {
                    has_passphrase: true,
                    ..
                } => store.get(&server_passphrase(&server.id)).unwrap_or_default(),
                _ => String::new(),
            };
            passwords.insert(server.id.clone(), password);
            private_key_passphrases.insert(server.id.clone(), passphrase);
        }
    }

    PanelState {
        groups: config
            .groups
            .iter()
            .map(|group| group_view(group, config.settings.language))
            .collect(),
        passwords,
        private_key_passphrases,
        ai_enabled: config.settings.ai.enabled,
        language: config.settings.language,
        show_hidden_files: config.settings.show_hidden_files,
        storage: storage(store),
    }
}

/// Save, then hand back the state the panel should now be showing.
///
/// Every mutating command ends this way. The alternative -- the page patching
/// what it already has -- is how a panel ends up disagreeing with the file it
/// is meant to be showing.
fn commit(config: &AppConfig, store: &Store) -> Result<PanelState, String> {
    config::save(config)?;
    Ok(panel_state(config, store))
}

/// Apply the dialog's fields to a server, keeping what the dialog does not own.
fn apply(server: &mut Server, input: &ServerInput) -> Result<(), String> {
    let host = input.host.trim();
    let username = input.username.trim();
    if host.is_empty() || username.is_empty() {
        return Err("hostUserRequired".to_string());
    }

    server.host = host.to_string();
    server.username = username.to_string();
    server.port = input.port.filter(|port| *port > 0).unwrap_or(22);
    server.name = match input.name.trim() {
        "" => host.to_string(),
        name => name.to_string(),
    };
    server.encoding = if input.encoding == "gb18030" {
        Encoding::Gb18030
    } else {
        Encoding::Utf8
    };

    if input.auth_type == "privateKey" {
        let path = input.private_key_path.trim();
        if path.is_empty() {
            return Err("privateKeyRequired".to_string());
        }
        server.auth = Auth::PrivateKey {
            path: path.to_string(),
            // Set by the caller, which is the only side that sees the secret.
            has_passphrase: false,
        };
    } else {
        server.auth = Auth::Password {
            has_password: false,
        };
    }
    Ok(())
}

/*
 * Write the two secrets to match the authentication that was just chosen, and
 * record in the config whether there is one.
 *
 * Both are written every time, including the one the chosen method does not use,
 * so that switching a server from a key back to a password does not leave the
 * passphrase behind in the keychain for nobody.
 */
fn store_secrets(
    server: &mut Server,
    store: &Store,
    password: &str,
    passphrase: &str,
) -> Result<(), String> {
    match &mut server.auth {
        Auth::Password { has_password } => {
            store.set(&server_password(&server.id), password)?;
            store.delete(&server_passphrase(&server.id))?;
            *has_password = !password.is_empty();
        }
        Auth::PrivateKey { has_passphrase, .. } => {
            store.set(&server_passphrase(&server.id), passphrase)?;
            store.delete(&server_password(&server.id))?;
            *has_passphrase = !passphrase.is_empty();
        }
    }
    Ok(())
}

// ------------------------------------------------------------- commands ---

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppInfo {
    name: &'static str,
    version: &'static str,
    platform: &'static str,
    language: String,
}

#[tauri::command]
fn app_info() -> AppInfo {
    // The configured language if there is a config to read, the machine's own
    // if there is not. A broken file falls back rather than failing the boot:
    // the panel is about to report the breakage, and it needs words to do it in.
    let language = config::load()
        .map(|config| config.settings.language)
        .unwrap_or_else(|_| Language::from_locale());

    AppInfo {
        name: "tshell",
        version: env!("CARGO_PKG_VERSION"),
        platform: std::env::consts::OS,
        language: language.tag().to_string(),
    }
}

#[tauri::command]
fn load_state(store: State<'_, Store>) -> Result<PanelState, String> {
    Ok(panel_state(&load()?, &store))
}

#[tauri::command]
fn add_group(name: String, store: State<'_, Store>) -> Result<PanelState, String> {
    let mut config = load()?;
    let name = match name.trim() {
        "" => i18n::t(config.settings.language, "newGroup"),
        trimmed => trimmed.to_string(),
    };
    config.groups.push(Group {
        id: config::make_id(),
        name,
        servers: Vec::new(),
    });
    commit(&config, &store)
}

#[tauri::command]
fn rename_group(
    group_id: String,
    name: String,
    store: State<'_, Store>,
) -> Result<PanelState, String> {
    let mut config = load()?;
    let fallback = i18n::t(config.settings.language, "newGroup");
    let group = config.group_mut(&group_id)?;
    group.name = match name.trim() {
        "" => fallback,
        trimmed => trimmed.to_string(),
    };
    commit(&config, &store)
}

#[tauri::command]
fn delete_group(group_id: String, store: State<'_, Store>) -> Result<PanelState, String> {
    let mut config = load()?;
    let doomed = config
        .groups
        .iter()
        .find(|group| group.id == group_id)
        .ok_or_else(|| "Group not found.".to_string())?;

    // The servers go with the group, so their secrets go too.
    for server in &doomed.servers {
        store.forget_server(&server.id);
    }

    config.groups.retain(|group| group.id != group_id);
    // normalize puts a default group back if that was the last one.
    config.normalize();
    commit(&config, &store)
}

#[tauri::command]
fn add_server(
    group_id: String,
    server: ServerInput,
    password: String,
    private_key_passphrase: String,
    store: State<'_, Store>,
) -> Result<PanelState, String> {
    let mut config = load()?;
    let mut next = Server {
        id: config::make_id(),
        name: String::new(),
        host: String::new(),
        port: 22,
        username: String::new(),
        encoding: Encoding::Utf8,
        auth: Auth::default(),
    };
    apply(&mut next, &server)?;
    store_secrets(&mut next, &store, &password, &private_key_passphrase)?;
    config.group_mut(&group_id)?.servers.push(next);
    commit(&config, &store)
}

#[tauri::command]
fn update_server(
    group_id: String,
    server: ServerInput,
    password: String,
    private_key_passphrase: String,
    store: State<'_, Store>,
) -> Result<PanelState, String> {
    let mut config = load()?;
    let group = config.group_mut(&group_id)?;
    let existing = group
        .servers
        .iter_mut()
        .find(|candidate| candidate.id == server.id)
        .ok_or_else(|| "Server not found.".to_string())?;

    apply(existing, &server)?;
    store_secrets(existing, &store, &password, &private_key_passphrase)?;
    commit(&config, &store)
}

#[tauri::command]
fn delete_server(
    group_id: String,
    server_id: String,
    store: State<'_, Store>,
) -> Result<PanelState, String> {
    let mut config = load()?;
    config.find_server(&group_id, &server_id).ok_or("Server not found.")?;
    store.forget_server(&server_id);
    config
        .group_mut(&group_id)?
        .servers
        .retain(|server| server.id != server_id);
    commit(&config, &store)
}

/*
 * The two drops the panel can produce. Both are ordinary edits to the config --
 * the order of a group in `groups`, and of a server in `servers`, is the order
 * the tree draws -- so neither needs a field the file did not already have.
 *
 * `store` is here only because `commit` reports back which secrets exist. No
 * secret is touched by either: a server keeps its id across a move, and its
 * keychain entries are filed under that id.
 */
#[tauri::command]
fn move_group(
    group_id: String,
    to_index: usize,
    store: State<'_, Store>,
) -> Result<PanelState, String> {
    let mut config = load()?;
    if !config.move_group(&group_id, to_index)? {
        // Dropped where it already was. Saying so with the current state beats
        // rewriting the file to prove nothing happened.
        return Ok(panel_state(&config, &store));
    }
    commit(&config, &store)
}

#[tauri::command]
fn move_server(
    from_group_id: String,
    server_id: String,
    to_group_id: String,
    to_index: usize,
    store: State<'_, Store>,
) -> Result<PanelState, String> {
    let mut config = load()?;
    if !config.move_server(&from_group_id, &server_id, &to_group_id, to_index)? {
        return Ok(panel_state(&config, &store));
    }
    commit(&config, &store)
}

// ------------------------------------------------------------- terminals ---

/// What `ssh::open` needs, assembled from the config and the keychain.
///
/// Kept separate from the command so that nothing borrowed from Tauri's state
/// is still held once the connecting starts: this reads what it needs, owns all
/// of it, and hands back something with no lifetimes in it.
pub(crate) fn dial_plan(
    group_id: &str,
    server_id: &str,
    store: &Store,
) -> Result<(ssh::Target, &'static encoding_rs::Encoding), String> {
    let config = load()?;
    let server = config
        .find_server(group_id, server_id)
        .ok_or("Server not found.")?;

    let encoding = match server.encoding {
        Encoding::Gb18030 => encoding_rs::GB18030,
        Encoding::Utf8 => encoding_rs::UTF_8,
    };

    let credential = match &server.auth {
        Auth::Password { .. } => {
            let password = store.get(&server_password(&server.id)).unwrap_or_default();
            // The extension asked for one at the prompt when none was stored.
            // There is nowhere to ask from yet, so this says which of the two
            // things went wrong rather than letting the server say "denied".
            if password.is_empty() {
                return Err("passwordRequired".to_string());
            }
            ssh::Credential::Password(password)
        }
        Auth::PrivateKey { path, .. } => ssh::Credential::Key {
            path: expand_home(path),
            passphrase: store.get(&server_passphrase(&server.id)),
        },
    };

    Ok((
        ssh::Target {
            host: server.host.clone(),
            port: server.port,
            username: server.username.clone(),
            encoding,
            credential,
            // The name the user gave the machine, because that is what they will
            // recognise in a host key dialog. An unnamed server is its host.
            label: if server.name.trim().is_empty() {
                server.host.clone()
            } else {
                server.name.clone()
            },
        },
        encoding,
    ))
}

/// `~` and `~/...` mean the home directory, as they do everywhere the user types
/// a path. Left alone if there is no home to expand against.
fn expand_home(value: &str) -> std::path::PathBuf {
    match (value, dirs::home_dir()) {
        ("~", Some(home)) => home,
        (path, Some(home)) if path.starts_with("~/") || path.starts_with("~\\") => {
            home.join(&path[2..])
        }
        (path, _) => std::path::PathBuf::from(path),
    }
}

/// The encoding a pane's server uses, for the input side.
///
/// Read from the config on each keystroke rather than remembered beside the
/// session, because it is one map lookup against a file that is already parsed,
/// and remembering it is one more thing that can disagree with the config.
fn encoding_of(group_id: &str, server_id: &str) -> &'static encoding_rs::Encoding {
    load()
        .ok()
        .and_then(|config| {
            config
                .find_server(group_id, server_id)
                .map(|server| match server.encoding {
                    Encoding::Gb18030 => encoding_rs::GB18030,
                    Encoding::Utf8 => encoding_rs::UTF_8,
                })
        })
        .unwrap_or(encoding_rs::UTF_8)
}

// ------------------------------------------------- terminal appearance ---

/*
 * The schemes the user owns, and the fonts they can name.
 *
 * The three schemes that ship with the product are not here -- they are in
 * `ui/shared/palettes.js`, because xterm is constructed with a JavaScript
 * object and there is no reading a Rust one from a canvas. What these commands
 * carry is the user's own schemes and their edits to the built-in ones; the
 * front end merges the two, in the one place both are already in scope.
 */

/// A file that will not parse is not an empty file. The error goes back as an
/// error so the settings page can say what is wrong and stop offering to save
/// over it -- overwriting a file we could not read is how a stray comma costs
/// somebody every scheme they made.
#[tauri::command]
fn schemes_load() -> Result<schemes::SchemesFile, String> {
    schemes::load().map_err(|error| error.message(&schemes::schemes_path()))
}

/// Returns what was written, normalized, which is what the next load will
/// produce. The settings page redraws from it rather than from what it sent,
/// so a colour it spelled loosely comes back spelled the one way.
#[tauri::command]
fn schemes_save(file: schemes::SchemesFile) -> Result<schemes::SchemesFile, String> {
    schemes::save(&file)
}

/// Off the main thread: the first call reads a table out of every font file on
/// the machine, and the window is not repainting while it does.
#[tauri::command]
async fn fonts_monospace() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| fonts::monospace().to_vec())
        .await
        .map_err(|error| error.to_string())
}

/// The same scan, unfiltered, for the window's own text.
#[tauri::command]
async fn fonts_all() -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(|| fonts::all().to_vec())
        .await
        .map_err(|error| error.to_string())
}

// ---------------------------------------------------- window appearance ---

/*
 * The window's palette, as the user's edits to it.
 *
 * The same division as the schemes above, and for a sharper reason: the palette
 * that ships is `ui/shared/theme.css`, a stylesheet, and the derived half of it
 * -- the hovers, the soft fills, the ink that goes on an accent -- is computed
 * in `ui/shared/palette.js` while a colour is still being dragged. These two
 * commands carry the fifteen tokens per half the user actually changed. What
 * they mean is the front end's.
 */

/// A file that will not parse is not an empty file. Same rule as the schemes
/// and the config: the error goes back, the file is left alone, and the window
/// wears the palette that ships until somebody fixes it.
#[tauri::command]
fn theme_load() -> Result<theme::ThemeFile, String> {
    theme::load().map_err(|error| error.message(&theme::theme_path()))
}

/// Returns what was written, normalized, which is what the next load will
/// produce -- so a colour typed as `#abc` comes back as `#AABBCC` and the
/// settings page never holds a second spelling of it.
#[tauri::command]
fn theme_save(file: theme::ThemeFile) -> Result<theme::ThemeFile, String> {
    theme::save(&file)
}

#[tauri::command]
async fn terminal_open(
    pane: String,
    group_id: String,
    server_id: String,
    cols: u32,
    rows: u32,
    on_event: Channel<InvokeResponseBody>,
    store: State<'_, Store>,
    sessions: State<'_, Arc<ssh::Sessions>>,
) -> Result<(), String> {
    // Both of these are resolved before the first await, so nothing borrowed
    // from the app's state is held while the connection is being made.
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let sessions = Arc::clone(&sessions);

    ssh::open(sessions, pane, target, cols.max(1), rows.max(1), on_event).await
}

/// Returns whether the session took it. A `false` is not an error -- it is a
/// closed connection, and the shell answers it by reconnecting on Enter.
#[tauri::command]
fn terminal_input(
    pane: String,
    group_id: String,
    server_id: String,
    data: String,
    sessions: State<'_, Arc<ssh::Sessions>>,
) -> bool {
    sessions.input(&pane, encoding_of(&group_id, &server_id), &data)
}

#[tauri::command]
fn terminal_resize(pane: String, cols: u32, rows: u32, sessions: State<'_, Arc<ssh::Sessions>>) {
    sessions.resize(&pane, cols.max(1), rows.max(1));
}

/// Re-read a session's encoding from the config and apply it in place.
///
/// Called when the server has just been edited. A terminal is a long-lived
/// thing and reconnecting it to change how its bytes are read would throw away
/// the scrollback and whatever was half-typed at the prompt.
#[tauri::command]
fn terminal_reload_encoding(
    pane: String,
    group_id: String,
    server_id: String,
    sessions: State<'_, Arc<ssh::Sessions>>,
) {
    sessions.set_encoding(&pane, encoding_of(&group_id, &server_id));
}

#[tauri::command]
fn terminal_close(pane: String, sessions: State<'_, Arc<ssh::Sessions>>) {
    sessions.close(&pane);
}

// ------------------------------------------------------------- transfers ---

/*
 * Every transfer command carries the server it is about rather than the pane
 * remembering one. The pane is a tab and the config is a file, and a tab that
 * cached a copy of a server would go on using the old host after the user
 * edited it. Looking it up costs one parse of a file that is already small.
 */
#[tauri::command]
async fn transfer_list(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    path: String,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<transfer::Listing, String> {
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    transfer::list(&transfers, &pane, &side, &target, &path).await
}

/// The parent of a path, worked out on the side that owns the path's rules.
///
/// Windows and POSIX disagree about separators, about what a root is, and about
/// whether there is anything above one -- going up from `C:\` reaches the drive
/// list, going up from `/` reaches `/`. Neither belongs in the front end.
#[tauri::command]
fn transfer_parent(side: String, path: String) -> String {
    if side == "local" {
        transfer::parent_local(&path)
    } else {
        transfer::parent_remote(&path)
    }
}

#[tauri::command]
fn transfer_join(side: String, base: String, name: String) -> String {
    if side == "local" {
        transfer::join_local(&base, &name)
    } else {
        transfer::join_remote(&base, &name)
    }
}

#[tauri::command]
fn transfer_default_local() -> String {
    transfer::default_local()
}

#[tauri::command]
fn transfer_set_hidden(
    pane: String,
    side: String,
    value: bool,
    transfers: State<'_, Arc<transfer::Transfers>>,
) {
    transfers.set_hidden(&pane, &side, value);
}

#[tauri::command]
async fn transfer_make_dir(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    path: String,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<(), String> {
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    transfer::make_dir(&transfers, &pane, &side, &target, &path).await
}

#[tauri::command]
async fn transfer_remove(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    paths: Vec<String>,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<(), String> {
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    transfer::remove(&transfers, &pane, &side, &target, &paths).await
}

#[tauri::command]
async fn transfer_rename(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    from: String,
    to: String,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<(), String> {
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    transfer::rename(&transfers, &pane, &side, &target, &from, &to).await
}

/// Which highlighter the panel should use, and -- for `dbf` -- which of the
/// two readers below to ask. Decided from the extension, in one place, so that
/// the page and the reader can never disagree about what a file is.
#[tauri::command]
fn preview_language(path: String) -> &'static str {
    preview::language(&path)
}

#[tauri::command]
async fn preview_text(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    path: String,
    encoding: String,
    offset: u64,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<preview::TextChunk, String> {
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    preview::text(&transfers, &pane, &side, &target, &path, &encoding, offset).await
}

#[tauri::command]
async fn preview_dbf(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    path: String,
    encoding: String,
    record_offset: u32,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<preview::DbfChunk, String> {
    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    preview::dbf(
        &transfers,
        &pane,
        &side,
        &target,
        &path,
        &encoding,
        record_offset,
    )
    .await
}

/// One root of a selection, as the page describes it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TransferRoot {
    path: String,
    #[serde(default)]
    is_directory: bool,
}

/*
 * The command runs for as long as the transfer does -- minutes, for a big tree.
 * That is deliberate: the page has one promise to await and one place to put the
 * summary, and everything in between arrives on the channel. Tauri runs each
 * command on its own task, so nothing else in the window waits on this.
 */
#[tauri::command]
async fn transfer_start(
    pane: String,
    side: String,
    group_id: String,
    server_id: String,
    items: Vec<TransferRoot>,
    target_dir: String,
    on_event: Channel<InvokeResponseBody>,
    store: State<'_, Store>,
    transfers: State<'_, Arc<transfer::Transfers>>,
) -> Result<transfer::Summary, String> {
    if items.is_empty() {
        return Err("nothingSelected".to_string());
    }
    if transfers.busy(&pane) {
        return Err("transferBusy".to_string());
    }

    let (target, _) = dial_plan(&group_id, &server_id, &store)?;
    let transfers = Arc::clone(&transfers);
    // The source side is what the page names; the other one follows from it.
    let to = if side == "local" { "remote" } else { "local" };
    let roots = items
        .into_iter()
        .map(|item| (item.path, item.is_directory))
        .collect();

    transfer::run(
        transfers,
        pane,
        side,
        to.to_string(),
        target,
        roots,
        target_dir,
        on_event,
    )
    .await
}

#[tauri::command]
fn transfer_cancel(pane: String, transfers: State<'_, Arc<transfer::Transfers>>) {
    transfers.cancel(&pane);
}

#[tauri::command]
fn transfer_answer(
    pane: String,
    id: u64,
    choice: String,
    transfers: State<'_, Arc<transfer::Transfers>>,
) {
    transfers.answer(&pane, id, &choice);
}

/// Polled by the page while it is open. True means a session that was alive is
/// not any more; the next request will open a fresh one.
#[tauri::command]
fn transfer_died(pane: String, transfers: State<'_, Arc<transfer::Transfers>>) -> bool {
    transfers.remote_died(&pane)
}

#[tauri::command]
fn transfer_close(pane: String, transfers: State<'_, Arc<transfer::Transfers>>) {
    transfers.close(&pane);
}

/// Hand the config file to whatever the desktop opens `.json` with.
///
/// The file is plain text and meant to be edited; the point of the menu entry is
/// to save the user hunting for it under an application data directory whose
/// location differs on all three platforms.
/// Which language everything is drawn in.
///
/// Its own command rather than a field on the AI patch, because it is not the
/// assistant's -- it names the string table the whole window reads. The page
/// reloads afterwards: every open tab was handed its labels in its bootstrap,
/// and there is no way to re-letter one without rebuilding it.
#[tauri::command]
fn set_language(language: Language, store: State<'_, Store>) -> Result<PanelState, String> {
    let mut config = load()?;
    config.settings.language = language;
    commit(&config, &store)
}

#[tauri::command]
fn open_config() -> Result<(), String> {
    let path = config::config_path();
    // Nothing has been saved yet on a first run, and opening a file that is not
    // there is a worse answer than writing the defaults out first.
    if !path.exists() {
        config::save(&AppConfig::fresh(Language::from_locale()))?;
    }
    reveal(&path).map_err(|error| error.to_string())
}

/// What the user pressed on a host key dialog.
///
/// Its own command rather than the result of the one that asked, because the
/// question is raised from inside a connection that a dozen different commands
/// may have started -- and, in a transfer job, from no command at all. The
/// question travels out as an event carrying an id; this brings the id back.
///
/// Answering an id nobody is waiting for does nothing: a dialog the window drew
/// twice, or an answer that arrives after the question timed out, is not an
/// error worth failing a call over.
#[tauri::command]
fn host_key_answer(id: u64, choice: String) {
    hosts::HOST_KEYS.answer(id, &choice);
}

pub(crate) fn reveal(path: &Path) -> std::io::Result<()> {
    use std::process::Command;
    #[cfg(target_os = "windows")]
    {
        // `start` is a shell builtin, not a program, hence the cmd. The empty
        // string is the window title argument, which `start` takes first and
        // would otherwise swallow the path if it were quoted.
        Command::new("cmd")
            .args(["/C", "start", "", &path.display().to_string()])
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    Command::new("open").arg(path).spawn()?;
    #[cfg(all(unix, not(target_os = "macos")))]
    Command::new("xdg-open").arg(path).spawn()?;
    Ok(())
}

fn main() {
    // Probed once, at startup, and shared: the fallback decision is about the
    // machine, not about any one password, and asking again per secret would
    // mean a panel that could disagree with itself about where its data is.
    let store = Store::open(&config::data_dir());

    tauri::Builder::default()
        .setup(|app| {
            // The front end shows the window once it has a frame up. This is the
            // net under that: a page that throws before reaching that call would
            // otherwise leave a process running with nothing on screen and no
            // way to reach it. Late and visible beats invisible.
            if let Some(window) = app.get_webview_window("main") {
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if !matches!(window.is_visible(), Ok(true)) {
                        let _ = window.show();
                    }
                });
            }
            /*
             * The host key dialog has somewhere to go from here on.
             *
             * Before this line every connection would be refused rather than
             * asked about -- see `hosts::HostKeys::ask`. Nothing can connect
             * before the window exists, so the window is the right moment.
             */
            hosts::HOST_KEYS.install(app.handle().clone());

            Ok(())
        })
        .manage(store)
        .manage(Arc::new(ssh::Sessions::default()))
        .manage(Arc::new(transfer::Transfers::default()))
        .manage(Arc::new(ai::bridge::Panels::default()))
        .manage(Arc::new(ai::bridge::AiStores::default()))
        .invoke_handler(tauri::generate_handler![
            app_info,
            load_state,
            add_group,
            rename_group,
            delete_group,
            add_server,
            update_server,
            delete_server,
            move_group,
            move_server,
            open_config,
            host_key_answer,
            set_language,
            schemes_load,
            schemes_save,
            fonts_monospace,
            fonts_all,
            theme_load,
            theme_save,
            terminal_open,
            terminal_input,
            terminal_resize,
            terminal_close,
            terminal_reload_encoding,
            transfer_list,
            transfer_parent,
            transfer_join,
            transfer_default_local,
            transfer_set_hidden,
            transfer_make_dir,
            transfer_remove,
            transfer_rename,
            transfer_close,
            transfer_died,
            transfer_start,
            transfer_cancel,
            transfer_answer,
            preview_language,
            preview_text,
            preview_dbf,
            ai::commands::ai_open,
            ai::commands::ai_send,
            ai::commands::ai_stop,
            ai::commands::ai_answer,
            ai::commands::ai_close,
            ai::commands::ai_history,
            ai::commands::ai_load_chat,
            ai::commands::ai_new_chat,
            ai::commands::ai_delete_chat,
            ai::commands::ai_memory_list,
            ai::commands::ai_memory_edit,
            ai::commands::ai_memory_delete,
            ai::commands::ai_memory_undo,
            ai::commands::ai_trust_list,
            ai::commands::ai_trust_add,
            ai::commands::ai_trust_remove,
            ai::commands::ai_skill_list,
            ai::commands::ai_skill_toggle,
            ai::commands::ai_model_list,
            ai::commands::ai_model_add,
            ai::commands::ai_model_delete,
            ai::commands::ai_model_select,
            ai::commands::ai_set_mode,
            ai::commands::ai_set_thinking,
            ai::commands::ai_settings_read,
            ai::commands::ai_settings_write,
            ai::commands::ai_reveal
        ])
        .run(tauri::generate_context!())
        .expect("tshell failed to start");
}
