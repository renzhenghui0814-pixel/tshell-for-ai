//! `tshell.config.json`: the file, its shape, and the rules that keep it honest.
//!
//! This is a redesign, not a port. The extension's file carried its passwords in
//! it, encrypted under a key derived from VS Code's machine id, which meant the
//! file could not be read by its owner, could not be moved to another machine,
//! and was still a file full of secrets. The secrets are gone from here now --
//! they live in [`crate::secrets`] -- and what is left is meant to be opened and
//! edited by hand.
//!
//! Three rules follow from that:
//!
//!  * **A version at the top.** A file written by a newer tshell is refused, not
//!    guessed at. Guessing is how a downgrade silently drops the fields it did
//!    not recognise and writes the loss back to disk.
//!  * **Authentication is a tagged union.** The old shape had `authType` beside
//!    `privateKeyPath` beside two encrypted blobs, and every reader had to know
//!    which combinations were real. `{"type": "privateKey", "path": ...}` cannot
//!    express the impossible ones.
//!  * **Nothing is overwritten on a parse error.** A stray comma is a reason to
//!    stop and say so; it is not a reason to replace the user's servers with an
//!    empty file, which is what the old loader did.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::atomic;

/// The only version this build reads or writes.
pub const VERSION: u32 = 2;

const DEFAULT_GROUP_ID: &str = "default";

/// How many servers the empty window offers to reopen.
pub const RECENT_MAX: usize = 5;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Language {
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en-US")]
    EnUs,
}

impl Language {
    pub fn tag(self) -> &'static str {
        match self {
            Language::ZhCn => "zh-CN",
            Language::EnUs => "en-US",
        }
    }

    /// What the machine's own locale suggests, narrowed to the two tables that
    /// exist. Only ever a first guess: the config file's setting wins.
    pub fn from_locale() -> Self {
        match sys_locale::get_locale() {
            Some(tag) if tag.to_lowercase().starts_with("zh") => Language::ZhCn,
            _ => Language::EnUs,
        }
    }
}

impl Default for Language {
    fn default() -> Self {
        Language::EnUs
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Encoding {
    #[serde(rename = "utf-8")]
    #[default]
    Utf8,
    #[serde(rename = "gb18030")]
    Gb18030,
}

/// How a server is authenticated, and whether the secret it needs is on file.
///
/// The `has*` flags say only that a secret exists in the keychain -- never what
/// it is. They are here so that the panel can tell "no password saved" from "a
/// password is saved" without unlocking the keychain to find out, and so that a
/// hand-edited file still describes what tshell will try.
/*
 * `rename_all` on an enum renames the variants -- `Password` to `password` --
 * and stops there. The fields inside them need `rename_all_fields`, and without
 * it this writes `has_password` into a file where everything else is camelCase
 * and then fails to read its own output back.
 */
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Auth {
    Password {
        #[serde(default)]
        has_password: bool,
    },
    PrivateKey {
        #[serde(default)]
        path: String,
        #[serde(default)]
        has_passphrase: bool,
    },
}

impl Default for Auth {
    fn default() -> Self {
        Auth::Password {
            has_password: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SshService {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub encoding: Encoding,
    #[serde(default)]
    pub auth: Auth,
}

fn default_port() -> u16 {
    22
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub servers: Vec<Service>,
}

/*
 * Local services carry only the properties their launcher consumes. They live
 * in the same ordered `servers` array as SSH services; `kind` is the sole
 * discriminator, so no parallel collection or order index can drift from it.
 */
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ShellService {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct VsDevService {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cwd: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind")]
pub enum Service {
    #[serde(rename = "SSH")]
    Ssh(SshService),
    #[serde(rename = "vsDev")]
    VsDev(VsDevService),
    #[serde(rename = "powerShell")]
    PowerShell(ShellService),
    #[serde(rename = "cmd")]
    Cmd(ShellService),
}

impl Service {
    pub fn id(&self) -> &str {
        match self {
            Service::Ssh(value) => &value.id,
            Service::VsDev(value) => &value.id,
            Service::PowerShell(value) | Service::Cmd(value) => &value.id,
        }
    }

    pub fn id_mut(&mut self) -> &mut String {
        match self {
            Service::Ssh(value) => &mut value.id,
            Service::VsDev(value) => &mut value.id,
            Service::PowerShell(value) | Service::Cmd(value) => &mut value.id,
        }
    }

    pub fn as_ssh(&self) -> Option<&SshService> {
        match self {
            Service::Ssh(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_ssh_mut(&mut self) -> Option<&mut SshService> {
        match self {
            Service::Ssh(value) => Some(value),
            _ => None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub show_hidden_files: bool,
    /// The assistant. See `ai/settings.rs` -- it holds no key, only where to
    /// find one.
    #[serde(default)]
    pub ai: crate::ai::settings::AiSettings,
    /*
     * Keyboard shortcuts, as the user's edits to them: action id to binding.
     *
     * Edits, not the table. A binding nobody changed is absent, so a later
     * build that picks a better default hands it to everyone who never opened
     * the panel -- the same rule the palette follows, for the same reason.
     *
     * What this file checks is the *shape* of a binding, and nothing else.
     * Whether a given combination is a good idea to bind -- that a bare letter
     * belongs to the shell on the far end, that the whole of `Ctrl` plus a
     * letter belongs to readline -- is one rule in `ui/shared/keys.js`, which
     * is both where it is enforced when the user picks one and where it is
     * enforced again before anything is matched against it. Written here too it
     * would be a second copy, and the direction two copies drift in is a
     * shortcut that this side stores and that side refuses to fire.
     */
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    /// Everything else under `settings`, carried through untouched.
    ///
    /// The one thing this must not do is drop what it does not recognise: a user
    /// who edits the file with a later tshell and opens it with this one should
    /// get their servers back, not a file with half their settings gone.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: Language::default(),
            show_hidden_files: false,
            ai: crate::ai::settings::AiSettings::default(),
            keys: BTreeMap::new(),
            rest: Map::new(),
        }
    }
}

/*
 * Whether a string is shaped like a binding: modifiers then a key, joined by
 * `+`, with the key half spelled as a `KeyboardEvent.code` -- `Ctrl+Shift+KeyT`,
 * `F11`, `Alt+Shift+Digit1`.
 *
 * A gate, not a policy. Anything that gets past it is a string the front end can
 * parse; whether it is a combination worth having is `keys.js`'s question. What
 * this stops is the other kind of damage: a hand-edited file putting arbitrary
 * text where a binding goes, which would otherwise be handed to a page and
 * compared against every keystroke forever.
 */
fn is_binding(text: &str) -> bool {
    if text.is_empty() || text.len() > 64 {
        return false;
    }
    let mut parts = text.split('+').collect::<Vec<_>>();
    let Some(code) = parts.pop() else {
        return false;
    };
    if parts.len() > 4 {
        return false;
    }
    let head_is_letter = code.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    if !head_is_letter || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    parts
        .iter()
        .all(|part| matches!(*part, "Ctrl" | "Alt" | "Meta" | "Shift"))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    #[serde(default)]
    pub groups: Vec<Group>,
    /// The last few servers opened, most recent first.
    ///
    /// Ids only. The position is the time -- a stamp beside it would be a
    /// second field saying the same thing, and two fields saying one thing can
    /// disagree. Left out of the file entirely while it is empty, so a config
    /// belonging to someone who has never opened a server is untouched by this.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub recent: Vec<String>,
}

impl AppConfig {
    pub fn fresh(language: Language) -> Self {
        let mut config = AppConfig {
            version: VERSION,
            settings: Settings {
                language,
                ..Settings::default()
            },
            groups: Vec::new(),
            recent: Vec::new(),
        };
        config.normalize();
        config
    }

    /// Put a server at the front of `recent`, having just been opened.
    ///
    /// An id no group holds is ignored rather than stored: the front of the
    /// list is where the eye goes first, and a pane that never opened has no
    /// claim on it.
    pub fn touch_recent(&mut self, id: &str) {
        let known = self.groups.iter().any(|group| {
            group
                .servers
                .iter()
                .any(|service| service.as_ssh().is_some_and(|server| server.id == id))
        });
        if !known {
            return;
        }
        self.recent.retain(|each| each != id);
        self.recent.insert(0, id.to_string());
        self.recent.truncate(RECENT_MAX);
    }

    /// Bring a parsed file up to what the rest of the code is allowed to assume.
    ///
    /// Everything here is a repair of something a hand-edited file can get
    /// wrong. It is deliberately quiet: a missing name is filled in, it is not
    /// an error, because refusing to open the panel over a blank field would
    /// make the file harder to edit by hand rather than easier.
    pub fn normalize(&mut self) {
        self.version = VERSION;
        self.settings.ai.normalize();
        self.settings.keys.retain(|_, binding| is_binding(binding));

        // The tree always needs a destination for a newly created service.
        if self.groups.is_empty() {
            self.groups.push(Group {
                id: DEFAULT_GROUP_ID.to_string(),
                name: String::new(),
                servers: Vec::new(),
            });
        }

        // Service ids identify sessions, secrets and open panes, so they are
        // unique across the whole file rather than merely within one group.
        let mut ids = std::collections::HashSet::new();
        for group in &mut self.groups {
            if group.id.trim().is_empty() {
                group.id = make_id();
            }
            /*
             * A blank group name is left blank on purpose. It is not missing
             * data -- it means "whatever this language calls the default
             * group", and it is resolved when the tree is drawn rather than
             * here. Writing the translation into the file would freeze the
             * name into the language that happened to be set the day the group
             * was made, which is the one thing this arrangement avoids.
             */
            group.servers.retain(|service| match service {
                Service::Ssh(server) => {
                    !server.host.trim().is_empty() && !server.username.trim().is_empty()
                }
                _ => true,
            });
            for service in &mut group.servers {
                if service.id().trim().is_empty() || !ids.insert(service.id().to_string()) {
                    let mut id = make_id();
                    while !ids.insert(id.clone()) {
                        id = make_id();
                    }
                    *service.id_mut() = id;
                }
                match service {
                    Service::Ssh(server) => {
                        if server.name.trim().is_empty() {
                            server.name = server.host.clone();
                        }
                        if server.port == 0 {
                            server.port = 22;
                        }
                        if matches!(&server.auth, Auth::PrivateKey { path, .. } if path.trim().is_empty())
                        {
                            server.auth = Auth::default();
                        }
                    }
                    Service::VsDev(term) => {
                        if term.name.trim().is_empty() {
                            term.name = "Visual Studio".to_string();
                        }
                        if term.arch.trim().is_empty() {
                            term.arch = "x86".to_string();
                        }
                    }
                    Service::PowerShell(term) => {
                        if term.name.trim().is_empty() {
                            term.name = "PowerShell".to_string();
                        }
                    }
                    Service::Cmd(term) => {
                        if term.name.trim().is_empty() {
                            term.name = "CMD".to_string();
                        }
                    }
                }
            }
        }

        /*
         * `recent` holds ids and nothing else, so an entry pointing at a
         * deleted server is an entry with nothing to draw. It is dropped here,
         * where the whole file is in hand, rather than left for each reader to
         * remember. Repeats go the same way: a hand-edited file is not the
         * authority on its own shape.
         */
        self.recent.retain(|id| {
            self.groups.iter().any(|group| {
                group
                    .servers
                    .iter()
                    .any(|service| service.as_ssh().is_some_and(|server| &server.id == id))
            })
        });
        let mut seen = std::collections::HashSet::new();
        self.recent.retain(|id| seen.insert(id.clone()));
        self.recent.truncate(RECENT_MAX);
    }

    pub fn group_mut(&mut self, id: &str) -> Result<&mut Group, String> {
        self.groups
            .iter_mut()
            .find(|group| group.id == id)
            .ok_or_else(|| "Group not found.".to_string())
    }

    /// Move a group to `to_index`, and say whether that was actually a move.
    ///
    /// `to_index` counts gaps in the list *as the panel is drawing it* -- the
    /// list the dragged group is still in. That is the only index the panel can
    /// name without knowing this rule, so the adjustment for the gap the group
    /// leaves behind belongs here rather than there. See [`resolve`].
    ///
    /// The `false` return is what keeps a drop-in-place off the disk: every
    /// caller commits, and committing writes the file.
    pub fn move_group(&mut self, group_id: &str, to_index: usize) -> Result<bool, String> {
        let from = self
            .groups
            .iter()
            .position(|group| group.id == group_id)
            .ok_or_else(|| "Group not found.".to_string())?;
        let Some(to) = resolve(from, to_index, self.groups.len()) else {
            return Ok(false);
        };
        let group = self.groups.remove(from);
        self.groups.insert(to, group);
        Ok(true)
    }

    /// Move any service within its group, or into another one.
    ///
    /// Both ends are located before anything is lifted out: a drop that names a
    /// group this build cannot find -- a stale panel, a group deleted while the
    /// drag was in the air -- has to leave the tree exactly as it was, and it
    /// cannot do that once the server is already out of its old group.
    pub fn move_service(
        &mut self,
        from_group_id: &str,
        service_id: &str,
        to_group_id: &str,
        to_index: usize,
    ) -> Result<bool, String> {
        let index_of = |wanted: &str| self.groups.iter().position(|group| group.id == wanted);
        let from_group = index_of(from_group_id).ok_or("Group not found.")?;
        let to_group = index_of(to_group_id).ok_or("Group not found.")?;
        let from = self.groups[from_group]
            .servers
            .iter()
            .position(|service| service.id() == service_id)
            .ok_or("Service not found.")?;

        if from_group == to_group {
            let servers = &mut self.groups[from_group].servers;
            let Some(to) = resolve(from, to_index, servers.len()) else {
                return Ok(false);
            };
            let service = servers.remove(from);
            servers.insert(to, service);
            return Ok(true);
        }

        /*
         * Across groups there is no gap to account for: the index is read
         * against the destination, which the service is not in yet. Clamping
         * rather than erroring, because the destination can legitimately have
         * shrunk since the panel drew it, and landing at the end is the answer
         * that loses nothing.
         */
        let service = self.groups[from_group].servers.remove(from);
        let servers = &mut self.groups[to_group].servers;
        let to = to_index.min(servers.len());
        servers.insert(to, service);
        Ok(true)
    }

    pub fn find_server(&self, group_id: &str, server_id: &str) -> Option<&SshService> {
        self.groups
            .iter()
            .find(|group| group.id == group_id)?
            .servers
            .iter()
            .find_map(|service| service.as_ssh().filter(|server| server.id == server_id))
    }

    pub fn find_local_term(&self, id: &str) -> Option<&Service> {
        self.groups
            .iter()
            .flat_map(|group| group.servers.iter())
            .find(|service| service.id() == id && service.as_ssh().is_none())
    }
}

/// Turn the index the panel names into the index the vector wants, or `None` if
/// the two describe the place the item is already in.
///
/// The panel counts the gaps between rows it can see, and the row being dragged
/// is one of them, so two of its indices mean "stay put": the item's own
/// position, and the gap immediately after it. Everything past that gap is one
/// too high once the item is lifted out.
fn resolve(from: usize, to_index: usize, len: usize) -> Option<usize> {
    let to = to_index.min(len);
    if to == from || to == from + 1 {
        return None;
    }
    Some(if to > from { to - 1 } else { to })
}

/// Enough randomness to not collide, short enough to read in the file.
pub fn make_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..12].to_string()
}

/// Where the file and the fallback secret store live.
///
/// Under the user's own data directory, the same place the extension's global
/// storage was, so that "my config is in a folder I can find" stays true.
pub fn data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("tshell")
}

pub fn config_path() -> PathBuf {
    data_dir().join("tshell.config.json")
}

#[derive(Debug)]
pub enum LoadError {
    /// Written by a newer tshell. Refused rather than guessed at.
    FutureVersion(u32),
    /// On disk but unreadable. The file is left exactly as it is.
    Unreadable(String),
}

impl LoadError {
    pub fn message(&self, path: &Path) -> String {
        match self {
            LoadError::FutureVersion(found) => format!(
                "{} was written by a newer tshell (version {found}; this build reads {VERSION}). \
                 Refusing to open it, so that nothing in it is lost.",
                path.display()
            ),
            LoadError::Unreadable(why) => format!(
                "{} could not be read: {why}. It has been left untouched -- fix or remove it.",
                path.display()
            ),
        }
    }
}

/// Read the config, or say why not.
///
/// A missing file is not a failure: it is a first run, and it produces a default
/// config that is not written to disk until something is actually saved. A file
/// that is present but broken *is* a failure, and one that leaves the file alone.
pub fn load() -> Result<AppConfig, LoadError> {
    let path = config_path();
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AppConfig::fresh(Language::from_locale()));
        }
        Err(error) => return Err(LoadError::Unreadable(error.to_string())),
    };

    let old_version = serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|value| value.get("version").and_then(Value::as_u64))
        .unwrap_or(VERSION as u64) as u32;
    let config = parse(&raw)?;
    if old_version == 1 {
        save(&config).map_err(LoadError::Unreadable)?;
    }
    Ok(config)
}

/// The half of [`load`] that has no filesystem in it, and so can be tested.
pub fn parse(raw: &str) -> Result<AppConfig, LoadError> {
    // The version is checked before the rest is interpreted: a future file may
    // well parse as this shape and mean something else by it.
    let mut probe: Value =
        serde_json::from_str(raw).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    let version = probe.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version > VERSION {
        return Err(LoadError::FutureVersion(version));
    }

    if version == 1 {
        migrate_v1(&mut probe);
    }

    let mut config: AppConfig =
        serde_json::from_value(probe).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    config.normalize();
    Ok(config)
}

/// Fold the transitional local-terminal model into the single ordered service
/// array. This runs once while loading a v1 file; `load` immediately writes the
/// resulting v2 shape, so no runtime code has to understand both models.
fn migrate_v1(root: &mut Value) {
    let Some(root) = root.as_object_mut() else {
        return;
    };
    let top_level_terms = root
        .remove("localTerms")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let groups = root
        .entry("groups")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut();
    let Some(groups) = groups else { return };

    if groups.is_empty() && !top_level_terms.is_empty() {
        groups.push(serde_json::json!({
            "id": DEFAULT_GROUP_ID,
            "name": "",
            "servers": []
        }));
    }

    for (index, group) in groups.iter_mut().enumerate() {
        let Some(group) = group.as_object_mut() else {
            continue;
        };
        let mut services = group
            .remove("servers")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        for service in &mut services {
            if let Some(service) = service.as_object_mut() {
                service.insert("kind".into(), Value::String("SSH".into()));
            }
        }
        services.extend(
            group
                .remove("localTerms")
                .and_then(|value| value.as_array().cloned())
                .unwrap_or_default(),
        );
        if index == 0 {
            services.extend(top_level_terms.clone());
        }

        let order = group
            .remove("order")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        if !order.is_empty() {
            let mut ordered = Vec::with_capacity(services.len());
            for wanted in order.iter().filter_map(Value::as_str) {
                if let Some(at) = services.iter().position(|service| service["id"] == wanted) {
                    ordered.push(services.remove(at));
                }
            }
            ordered.append(&mut services);
            services = ordered;
        }
        group.insert("servers".into(), Value::Array(services));
    }
    root.insert("version".into(), Value::from(VERSION));
}

pub fn save(config: &AppConfig) -> Result<(), String> {
    let body = serde_json::to_string_pretty(config).map_err(|error| error.to_string())?;
    atomic::write(&config_path(), &format!("{body}\n")).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(raw: &str) -> AppConfig {
        parse(raw).expect("should parse")
    }

    /*
     * The shape gate on a binding, and what it deliberately does not do.
     *
     * `KeyA` gets through here: it is shaped like a binding, and whether a bare
     * letter is a sane thing to bind is `keys.js`'s question, asked both when
     * the user picks one and again before anything is matched. What must not
     * get through is text that is not a binding at all, because that is what
     * ends up compared against every keystroke in every page.
     */
    #[test]
    fn a_binding_is_checked_for_shape_and_not_for_sense() {
        for good in [
            "Ctrl+Shift+KeyT",
            "F11",
            "Alt+Shift+Digit1",
            "KeyA",
            "Meta+Space",
        ] {
            assert!(is_binding(good), "{good} should be a binding");
        }
        for bad in [
            "",
            "Ctrl+",
            "+KeyT",
            "Hyper+KeyT",
            "Ctrl+Shift+1Key",
            "Ctrl Shift T",
        ] {
            assert!(!is_binding(bad), "{bad:?} should not be a binding");
        }
        // Long enough to be someone's essay rather than someone's keystroke.
        assert!(!is_binding(&"A".repeat(65)));
    }

    /// A binding that is not one is dropped on the way in, the same as a colour
    /// that is not a colour. The rest of the map is not disturbed by it.
    #[test]
    fn an_unreadable_binding_is_dropped() {
        let raw = r#"{"version":2,"settings":{"keys":{"openTransfer":"Ctrl+Shift+KeyT","openAssistant":"nonsense here"}},"groups":[]}"#;
        let config = parse_ok(raw);
        assert_eq!(
            config.settings.keys.get("openTransfer").map(String::as_str),
            Some("Ctrl+Shift+KeyT")
        );
        assert!(!config.settings.keys.contains_key("openAssistant"));
    }

    /// The point of the version field. A file from a later build is refused, not
    /// read as best it can be and written back with whatever it did not
    /// understand quietly dropped.
    #[test]
    fn refuses_a_future_version() {
        let raw = r#"{"version": 3, "settings": {}, "groups": []}"#;
        assert!(matches!(parse(raw), Err(LoadError::FutureVersion(3))));
    }

    /// A broken file is an error, and specifically not a reason to start over.
    /// `load` never writes on this path, which is what keeps the user's servers.
    #[test]
    fn refuses_a_broken_file() {
        assert!(matches!(parse("{ not json"), Err(LoadError::Unreadable(_))));
    }

    #[test]
    fn tagged_auth_survives_a_round_trip() {
        let raw = r#"{
          "version": 2,
          "settings": { "language": "zh-CN", "showHiddenFiles": true },
          "groups": [{ "id": "g", "name": "G", "servers": [
            { "id": "a", "kind": "SSH", "name": "a", "host": "h1", "port": 22, "username": "u",
              "encoding": "gb18030", "auth": { "type": "password", "hasPassword": true } },
            { "id": "b", "kind": "SSH", "name": "b", "host": "h2", "port": 2222, "username": "u",
              "encoding": "utf-8",
              "auth": { "type": "privateKey", "path": "~/.ssh/id_ed25519", "hasPassphrase": true } }
          ]}]
        }"#;

        let config = parse_ok(raw);
        let again = parse_ok(&serde_json::to_string(&config).unwrap());

        let servers = &again.groups[0].servers;
        assert_eq!(
            servers[0].as_ssh().unwrap().auth,
            Auth::Password { has_password: true }
        );
        assert_eq!(servers[0].as_ssh().unwrap().encoding, Encoding::Gb18030);
        assert_eq!(
            servers[1].as_ssh().unwrap().auth,
            Auth::PrivateKey {
                path: "~/.ssh/id_ed25519".into(),
                has_passphrase: true
            }
        );
        assert_eq!(again.settings.language, Language::ZhCn);
        assert!(again.settings.show_hidden_files);
    }

    /// No secret may reach the file, by any route. This is the invariant the
    /// whole redesign exists for, so it is asserted on the serialised bytes
    /// rather than on the struct that produced them.
    #[test]
    fn the_written_file_holds_no_secret() {
        let config = parse_ok(
            r#"{"version":2,"groups":[{"id":"g","name":"G","servers":[
                 {"id":"a","kind":"SSH","name":"a","host":"h","username":"u",
                  "auth":{"type":"password","hasPassword":true}}]}]}"#,
        );
        let written = serde_json::to_string(&config).unwrap();
        assert!(written.contains("hasPassword"));
        assert!(!written.contains("password\":\""));
        assert!(!written.contains("enc:v1"));
    }

    /// Settings this build has never heard of belong to a later one. Dropping
    /// them would turn opening the file in an older tshell into data loss.
    #[test]
    fn unknown_settings_are_carried_through() {
        let config = parse_ok(
            r#"{"version":2,"settings":{"language":"en-US","fromALaterBuild":{"x":1}},"groups":[]}"#,
        );
        let written = serde_json::to_string(&config).unwrap();
        assert!(
            written.contains(r#""fromALaterBuild":{"x":1}"#),
            "{written}"
        );
    }

    /// The same courtesy one level down: `ai` is understood now, but a key
    /// inside it that this build has never heard of is still a later build's.
    #[test]
    fn unknown_ai_settings_are_carried_through() {
        let config = parse_ok(
            r#"{"version":2,"settings":{"ai":{"enabled":true,"fromALaterBuild":2}},"groups":[]}"#,
        );
        let written = serde_json::to_string(&config).unwrap();
        assert!(written.contains(r#""fromALaterBuild":2"#), "{written}");
    }

    #[test]
    fn normalize_repairs_what_a_hand_edit_can_break() {
        let config = parse_ok(
            r#"{"version":2,"groups":[{"id":"g","name":"G","servers":[
                 {"id":"","kind":"SSH","name":"","host":"only-a-host","username":"u"},
                 {"id":"empty-host","kind":"SSH","name":"bad","host":"","username":"u"},
                 {"id":"empty-user","kind":"SSH","name":"bad","host":"h","username":""},
                 {"id":"k","kind":"SSH","name":"k","host":"h","username":"u","port":0,
                  "auth":{"type":"privateKey","path":"  "}}]}]}"#,
        );

        let servers = &config.groups[0].servers;
        // The two with nothing to connect to are gone; the two usable ones stay.
        assert_eq!(servers.len(), 2);

        let first = servers[0].as_ssh().unwrap();
        assert!(!first.id.is_empty(), "a missing id is filled in");
        assert_eq!(first.name, "only-a-host", "the name falls back to the host");
        assert_eq!(first.port, 22);

        // A private key with no path cannot connect and cannot be repaired here.
        let second = servers[1].as_ssh().unwrap();
        assert_eq!(second.port, 22, "port 0 is not a port");
        assert_eq!(
            second.auth,
            Auth::Password {
                has_password: false
            }
        );
    }

    /// The tree draws groups, so there has to be one to drop a server into.
    ///
    /// Its name is blank, and that is the point: blank resolves to whatever the
    /// current language calls the default group, so switching languages relabels
    /// it. Writing the translation in here would freeze it into the language
    /// that happened to be set on the day the file was created.
    #[test]
    fn an_empty_file_still_has_a_blank_named_group() {
        let config = parse_ok(r#"{"version":2,"groups":[]}"#);
        assert_eq!(config.groups.len(), 1);
        assert_eq!(config.groups[0].id, DEFAULT_GROUP_ID);
        assert!(config.groups[0].name.is_empty());
    }

    /// The default group renames like any other.
    ///
    /// It did not, and the failure was invisible from the outside: the new name
    /// was saved to the file correctly and then drawn over, because the tree
    /// labelled any group whose id was `default` with the translated string
    /// rather than with its name. Nothing here was wrong, so nothing here would
    /// have caught it -- hence the test, on the half that can be tested.
    #[test]
    fn a_renamed_default_group_keeps_its_name() {
        let config =
            parse_ok(r#"{"version":2,"groups":[{"id":"default","name":"生产","servers":[]}]}"#);
        assert_eq!(config.groups[0].name, "生产");
    }

    /// A file with no version at all is this version -- that is what `default`
    /// on the field means -- and it must not be mistaken for a future one.
    #[test]
    fn a_missing_version_reads_as_current() {
        let config = parse_ok(r#"{"groups":[]}"#);
        assert_eq!(config.version, VERSION);
    }

    // -------------------------------------------------------- reordering ---

    /// Two groups of two, named so that a move is legible in one assert.
    fn tree() -> AppConfig {
        parse_ok(
            r#"{"version":2,"groups":[
                 {"id":"g1","name":"G1","servers":[
                   {"id":"a","kind":"SSH","name":"a","host":"a","username":"u"},
                   {"id":"b","kind":"SSH","name":"b","host":"b","username":"u"}]},
                 {"id":"g2","name":"G2","servers":[
                   {"id":"c","kind":"SSH","name":"c","host":"c","username":"u"},
                   {"id":"d","kind":"SSH","name":"d","host":"d","username":"u"}]}]}"#,
        )
    }

    fn group_ids(config: &AppConfig) -> Vec<&str> {
        config.groups.iter().map(|g| g.id.as_str()).collect()
    }

    fn server_ids<'a>(config: &'a AppConfig, group_id: &str) -> Vec<&'a str> {
        config
            .groups
            .iter()
            .find(|g| g.id == group_id)
            .expect("group")
            .servers
            .iter()
            .map(Service::id)
            .collect()
    }

    #[test]
    fn a_group_moves_up() {
        let mut config = tree();
        assert!(config.move_group("g2", 0).unwrap());
        assert_eq!(group_ids(&config), ["g2", "g1"]);
    }

    /// Dragging downwards is where the index the panel sends and the index the
    /// vector wants come apart: the panel counts gaps in the list it can see,
    /// which still contains the thing being dragged. Asking for "position 2" of
    /// [g1, g2, g3] means after g2 -- and once g1 is lifted out, that is 1.
    #[test]
    fn a_downward_move_accounts_for_the_gap_it_leaves() {
        let mut config = parse_ok(
            r#"{"version":2,"groups":[
                 {"id":"g1","name":"","servers":[]},
                 {"id":"g2","name":"","servers":[]},
                 {"id":"g3","name":"","servers":[]}]}"#,
        );
        assert!(config.move_group("g1", 2).unwrap());
        assert_eq!(group_ids(&config), ["g2", "g1", "g3"]);
    }

    #[test]
    fn a_group_dropped_where_it_already_is_changes_nothing() {
        let mut config = tree();
        // Both the index it occupies and the gap just after it: neither moves it.
        assert!(!config.move_group("g1", 0).unwrap());
        assert!(!config.move_group("g1", 1).unwrap());
        assert_eq!(group_ids(&config), ["g1", "g2"]);
    }

    #[test]
    fn an_index_past_the_end_lands_at_the_end() {
        let mut config = tree();
        assert!(config.move_group("g1", 99).unwrap());
        assert_eq!(group_ids(&config), ["g2", "g1"]);
    }

    #[test]
    fn a_server_reorders_inside_its_group() {
        let mut config = tree();
        assert!(config.move_service("g1", "b", "g1", 0).unwrap());
        assert_eq!(server_ids(&config, "g1"), ["b", "a"]);
    }

    /// The move that changes which group a server belongs to. Its id is the same
    /// id afterwards, which is what leaves its keychain entries alone -- they are
    /// filed under the server, and the server did not become a different one.
    #[test]
    fn a_server_moves_to_another_group() {
        let mut config = tree();
        assert!(config.move_service("g1", "a", "g2", 1).unwrap());
        assert_eq!(server_ids(&config, "g1"), ["b"]);
        assert_eq!(server_ids(&config, "g2"), ["c", "a", "d"]);
    }

    #[test]
    fn every_service_kind_reorders_and_moves_through_the_same_array() {
        let mut config = parse_ok(
            r#"{"version":2,"groups":[
              {"id":"g1","name":"One","servers":[
                {"id":"cmd","name":"CMD","kind":"cmd"},
                {"id":"ps","name":"PowerShell","kind":"powerShell"}]},
              {"id":"g2","name":"Two","servers":[
                {"id":"vs","name":"VS","kind":"vsDev"}]}
            ]}"#,
        );
        assert!(config.move_service("g1", "ps", "g1", 0).unwrap());
        assert_eq!(server_ids(&config, "g1"), ["ps", "cmd"]);
        assert!(config.move_service("g1", "cmd", "g2", 1).unwrap());
        assert_eq!(server_ids(&config, "g2"), ["vs", "cmd"]);
    }

    #[test]
    fn a_server_dropped_where_it_already_is_changes_nothing() {
        let mut config = tree();
        assert!(!config.move_service("g1", "a", "g1", 0).unwrap());
        assert!(!config.move_service("g1", "a", "g1", 1).unwrap());
        assert_eq!(server_ids(&config, "g1"), ["a", "b"]);
    }

    /// A cross-group drop is never a no-op, even at the index the server appears
    /// to already hold: the same number in a different group is a different place.
    #[test]
    fn the_same_index_in_another_group_is_still_a_move() {
        let mut config = tree();
        assert!(config.move_service("g1", "a", "g2", 0).unwrap());
        assert_eq!(server_ids(&config, "g1"), ["b"]);
        assert_eq!(server_ids(&config, "g2"), ["a", "c", "d"]);
    }

    /// Nothing is half-done on a bad id. The panel and the file can disagree --
    /// a stale drop, a group deleted in between -- and the answer is to refuse,
    /// not to lift a server out and then discover there is nowhere to put it.
    #[test]
    fn a_move_to_a_missing_group_leaves_the_tree_alone() {
        let mut config = tree();
        assert!(config.move_service("g1", "a", "nope", 0).is_err());
        assert!(config.move_service("g1", "nope", "g2", 0).is_err());
        assert!(config.move_group("nope", 0).is_err());
        assert_eq!(server_ids(&config, "g1"), ["a", "b"]);
        assert_eq!(server_ids(&config, "g2"), ["c", "d"]);
    }

    // ------------------------------------------------------------ recent ---

    /// Enough servers that the cap on `recent` has something to cut.
    fn seven_servers() -> AppConfig {
        let servers: Vec<String> = (1..=7)
            .map(|n| {
                format!(
                    r#"{{"id":"s{n}","kind":"SSH","name":"s{n}","host":"h{n}","username":"u"}}"#
                )
            })
            .collect();
        parse_ok(&format!(
            r#"{{"version":2,"groups":[{{"id":"g","name":"G","servers":[{}]}}]}}"#,
            servers.join(",")
        ))
    }

    /// The list is an order, not a set of stamps: touching a server that is
    /// already in it moves it to the front rather than adding a second copy.
    /// That is the whole reason no timestamp is stored -- the position is the
    /// time, and a second field saying the same thing is a second field that
    /// can disagree.
    #[test]
    fn touching_a_server_moves_it_to_the_front() {
        let mut config = tree();
        config.touch_recent("a");
        config.touch_recent("b");
        config.touch_recent("a");
        assert_eq!(config.recent, ["a", "b"]);
    }

    /// Five is what the empty window has room for, so the sixth pushes the
    /// oldest out here rather than leaving the drawing side to decide how much
    /// of a list it was given to ignore.
    #[test]
    fn recent_keeps_only_the_last_five() {
        let mut config = seven_servers();
        for id in ["s1", "s2", "s3", "s4", "s5", "s6", "s7"] {
            config.touch_recent(id);
        }
        assert_eq!(config.recent, ["s7", "s6", "s5", "s4", "s3"]);
    }

    /// The cap holds on the way in as well as on the way through: a file with
    /// forty entries in it is trimmed when it is read, not when it is drawn.
    #[test]
    fn normalize_trims_a_long_recent_list() {
        let mut config = seven_servers();
        config.recent = (1..=7).map(|n| format!("s{n}")).collect();
        config.normalize();
        assert_eq!(config.recent, ["s1", "s2", "s3", "s4", "s5"]);
    }

    /// A server that was deleted leaves an id behind, and the id is the only
    /// thing stored -- there is no name in the list to draw. Dropping it here,
    /// where the whole file is in hand, is cheaper than every reader having to
    /// remember that a `recent` entry may point at nothing.
    #[test]
    fn normalize_drops_recent_ids_no_group_holds() {
        let mut config = tree();
        config.recent = vec!["a".into(), "gone".into(), "c".into()];
        config.normalize();
        assert_eq!(config.recent, ["a", "c"]);
    }

    /// A hand-edited file can repeat an id; the file is not the authority on
    /// its own shape.
    #[test]
    fn normalize_removes_a_repeated_recent_id() {
        let mut config = tree();
        config.recent = vec!["a".into(), "b".into(), "a".into()];
        config.normalize();
        assert_eq!(config.recent, ["a", "b"]);
    }

    /// Touching an id no group holds does nothing. The front of the list is
    /// where the eye goes first, and a pane that failed to open has no claim
    /// on it.
    #[test]
    fn touching_an_unknown_server_changes_nothing() {
        let mut config = tree();
        config.touch_recent("a");
        config.touch_recent("nope");
        assert_eq!(config.recent, ["a"]);
    }

    // ----------------------------------------------------------- services ---

    #[test]
    fn v1_is_folded_into_one_array_in_its_recorded_order() {
        let config = parse_ok(
            r#"{"version":1,"groups":[{"id":"g","name":"Mixed","servers":[
              {"id":"ssh","name":"Remote","host":"host","username":"user"}
            ],"localTerms":[
              {"id":"cmd","kind":"cmd","name":"CMD"},
              {"id":"ps","kind":"powerShell","name":"PowerShell"}
            ],"order":["cmd","ssh","ps"]}]}"#,
        );
        assert_eq!(server_ids(&config, "g"), ["cmd", "ssh", "ps"]);
        assert!(matches!(config.groups[0].servers[0], Service::Cmd(_)));
        assert!(matches!(config.groups[0].servers[1], Service::Ssh(_)));

        let written = serde_json::to_value(&config).unwrap();
        assert_eq!(written["version"], VERSION);
        assert!(written["groups"][0].get("localTerms").is_none());
        assert!(written["groups"][0].get("order").is_none());
        assert_eq!(written["groups"][0]["servers"][1]["kind"], "SSH");
    }

    #[test]
    fn all_service_kinds_share_one_ordered_array_and_round_trip() {
        let raw = r#"{
          "version": 2,
          "groups": [{
            "id": "g", "name": "Mixed", "servers": [
              { "id": "ssh", "kind": "SSH", "name": "Remote", "host": "host",
                "port": 22, "username": "user" },
              { "id": "cmd", "kind": "cmd", "name": "CMD", "cwd": "C:\\src" },
              { "id": "ps", "kind": "powerShell", "name": "PowerShell", "cwd": "D:\\work" },
              { "id": "vs", "kind": "vsDev", "name": "VS 2013 x86",
                "path": "D:\\VS\\vcvarsall.bat", "arch": "x86", "cwd": "D:\\code" }
            ]
          }]
        }"#;

        let config = parse_ok(raw);
        assert_eq!(server_ids(&config, "g"), ["ssh", "cmd", "ps", "vs"]);
        assert!(matches!(config.groups[0].servers[0], Service::Ssh(_)));
        assert!(matches!(config.groups[0].servers[1], Service::Cmd(_)));
        assert!(matches!(
            config.groups[0].servers[2],
            Service::PowerShell(_)
        ));
        assert!(matches!(config.groups[0].servers[3], Service::VsDev(_)));

        let written = serde_json::to_value(&config).unwrap();
        assert!(written.get("localTerms").is_none());
        let group = &written["groups"][0];
        assert!(group.get("order").is_none());
        let kinds: Vec<&str> = group["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|service| service["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["SSH", "cmd", "powerShell", "vsDev"]);
        assert!(parse(&serde_json::to_string(&written).unwrap()).is_ok());
    }

    #[test]
    fn service_ids_are_made_unique_across_all_groups() {
        let config = parse_ok(
            r#"{"version":2,"groups":[
              {"id":"g1","name":"G1","servers":[
                {"id":"a","kind":"cmd","name":"one"}
              ]},
              {"id":"g2","name":"G2","servers":[
                {"id":"a","kind":"powerShell","name":"two"},
                {"id":"","kind":"vsDev","name":"three"}
              ]}
            ]}"#,
        );
        let first = server_ids(&config, "g1");
        let second = server_ids(&config, "g2");
        assert_eq!(first[0], "a");
        assert_ne!(second[0], "a");
        assert!(!second[0].is_empty());
        assert!(!second[1].is_empty());
        assert_ne!(second[0], second[1]);
    }

    #[test]
    fn a_local_service_is_found_by_id_inside_any_group() {
        let config = parse_ok(
            r#"{"version":2,"groups":[
              {"id":"g1","name":"One","servers":[]},
              {"id":"g2","name":"Two","servers":[
                {"id":"t","kind":"cmd","name":"CMD","cwd":"C:\\src"}
              ]}
            ]}"#,
        );
        let term = config.find_local_term("t").expect("found by id");
        assert_eq!(term.id(), "t");
        assert!(matches!(term, Service::Cmd(_)));
        assert!(config.find_local_term("nope").is_none());
    }
}
