//! Where the secrets live, and how the user finds out when that is not the keychain.
//!
//! The config file is meant to be readable -- opened, diffed, edited by hand, put
//! under version control if you like -- and that is only defensible if nothing in
//! it is secret. So passwords, private key passphrases and API keys go to the
//! platform's own store instead: Credential Manager on Windows, Keychain on
//! macOS, Secret Service on Linux.
//!
//! Linux is the one that can be missing. A headless box, or a desktop session
//! with no keyring daemon running, has nowhere to put a password, and the choice
//! is between refusing to store one at all and storing it somewhere weaker. This
//! falls back to a file readable only by its owner -- and says so. A silent
//! downgrade from "the operating system is protecting this" to "a file
//! permission is protecting this" is exactly the sort of thing a user has to be
//! told about, so [`Store::backend`] is carried all the way to the settings page
//! rather than logged where nobody looks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::atomic;

/// What the keychain files these under. One service, many named entries.
const SERVICE: &str = "tshell";

/// The key a server's password is stored at.
pub fn server_password(id: &str) -> String {
    format!("server/{id}/password")
}

/// The key a server's private key passphrase is stored at.
pub fn server_passphrase(id: &str) -> String {
    format!("server/{id}/passphrase")
}

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum Backend {
    /// The platform's own credential store. What every supported platform
    /// should be using.
    Keychain,
    /// A file with owner-only permissions, because there was no credential
    /// store to talk to. Weaker, and shown as such in the settings page.
    File,
}

pub struct Store {
    backend: Backend,
    /// Only meaningful for [`Backend::File`].
    path: PathBuf,
    /// Why the keychain was unavailable, for the settings page to show.
    reason: Option<String>,
}

impl Store {
    /// Probe the platform store once at startup and settle on a backend.
    ///
    /// `store_status` initialises the credential store without touching any
    /// credential, so this costs one probe and never invents an entry just to
    /// find out whether it could.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("secrets.json");
        match keyring::Entry::store_status() {
            Ok(()) => Store {
                backend: Backend::Keychain,
                path,
                reason: None,
            },
            Err(error) => Store {
                backend: Backend::File,
                path,
                reason: Some(error.to_string()),
            },
        }
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// The fallback file's path. Named even when it is not in use, so the
    /// settings page can say where the secrets *would* go.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The secret at `key`, or `None` when there is none.
    ///
    /// A store that errors is reported as empty rather than as a failure. The
    /// callers are all "fill in the password field" and "connect with what we
    /// have": none of them can do anything useful with the difference between
    /// an absent password and an unreachable one, and treating the second as
    /// fatal would lock a user out of a server they could otherwise type their
    /// way into.
    pub fn get(&self, key: &str) -> Option<String> {
        match self.backend {
            Backend::Keychain => keyring::Entry::new(SERVICE, key)
                .and_then(|entry| entry.get_password())
                .ok(),
            Backend::File => self.read_file().remove(key),
        }
    }

    /// Store `value` at `key`, or remove the entry when `value` is empty.
    ///
    /// Empty means absent throughout: the dialogs send back "" for a field the
    /// user cleared, and an empty password is not a password.
    pub fn set(&self, key: &str, value: &str) -> Result<(), String> {
        if value.is_empty() {
            return self.delete(key);
        }
        match self.backend {
            Backend::Keychain => keyring::Entry::new(SERVICE, key)
                .and_then(|entry| entry.set_password(value))
                .map_err(|error| error.to_string()),
            Backend::File => {
                let mut all = self.read_file();
                all.insert(key.to_string(), value.to_string());
                self.write_file(&all)
            }
        }
    }

    pub fn delete(&self, key: &str) -> Result<(), String> {
        match self.backend {
            Backend::Keychain => match keyring::Entry::new(SERVICE, key) {
                Ok(entry) => match entry.delete_credential() {
                    // Deleting what was never there is the outcome asked for.
                    Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                    Err(error) => Err(error.to_string()),
                },
                Err(keyring::Error::NoEntry) => Ok(()),
                Err(error) => Err(error.to_string()),
            },
            Backend::File => {
                let mut all = self.read_file();
                if all.remove(key).is_none() {
                    return Ok(());
                }
                self.write_file(&all)
            }
        }
    }

    /// Forget everything belonging to one server. Called when it is deleted, so
    /// that removing a server removes its password with it rather than leaving
    /// an orphan in the keychain that nothing will ever look at again.
    pub fn forget_server(&self, id: &str) {
        let _ = self.delete(&server_password(id));
        let _ = self.delete(&server_passphrase(id));
    }

    fn read_file(&self) -> BTreeMap<String, String> {
        std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn write_file(&self, all: &BTreeMap<String, String>) -> Result<(), String> {
        let body = serde_json::to_string_pretty(all).map_err(|error| error.to_string())?;
        atomic::write_private(&self.path, &format!("{body}\n")).map_err(|error| error.to_string())
    }
}
