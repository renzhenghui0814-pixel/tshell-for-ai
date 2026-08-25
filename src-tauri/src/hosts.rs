//! `tshell.hosts.json`: which key each machine answered with last time, and the
//! question asked when that changes.
//!
//! # Why this file exists at all
//!
//! Until now [`crate::ssh::Client`] returned `true` from `check_server_key` to
//! everything, which made every connection confidential but not authenticated:
//! anyone able to answer on the address could present their own key, relay to the
//! real server, and read the password and every keystroke in between. That is a
//! worse trade here than in a plain terminal, because the assistant also sends
//! what it sees to a model endpoint -- someone in the middle gets both what is on
//! the machine and what the user is trying to do with it.
//!
//! # Trust on first use, and what "first" means
//!
//! There is no authority to ask, so the first key a machine offers is the one it
//! is thereafter held to. The user is shown the fingerprint and asked once; from
//! then on a match is silent and a mismatch is loud. This is what `ssh` itself
//! does, and it is the whole of the protection: it does not tell you the first
//! key was genuine, it tells you nothing has changed since.
//!
//! # This is not `known_hosts`
//!
//! OpenSSH's two files are still left alone -- neither read nor written -- so
//! nothing here changes what `ssh` on the command line does, in either
//! direction. Sharing the file would mean matching its hashed hostnames, its
//! wildcards, its markers and its certificate authorities, and getting any of
//! that subtly wrong is worse than keeping our own: a `@cert-authority` line
//! misread as an ordinary key would refuse every machine in an estate that works
//! perfectly. Our file is small, plain, and answers one question.
//!
//! # A file we cannot read is not a file we may overwrite
//!
//! The rule from [`crate::config`] holds here for a sharper reason. If the file
//! is corrupt, every machine comes out unknown and the user is asked again --
//! annoying, and safe. If we instead started a fresh file, the first "trust"
//! would silently throw away every key ever pinned, which is exactly the state
//! an attacker would like us in. So a load error stops us writing until it is
//! fixed, and the reason travels to the dialog so the user knows why they are
//! being asked about a machine they have used for months.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter};
use tokio::sync::oneshot;

use crate::atomic;
use crate::config;

/// The only version this build reads or writes.
pub const VERSION: u32 = 1;

/// The event the window's shell listens for. One event, one question.
pub const ASK_EVENT: &str = "host-key";

/// How long a question may stand before it counts as declined.
///
/// Not a guess at how slowly people read: it is there so that a window which
/// reloaded, crashed, or was never listening cannot leave a half-open connection
/// and a task waiting on an answer that is never coming. Long enough that nobody
/// actually looking at the dialog will ever meet it.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(180);

// ------------------------------------------------------------------- file ---

/// One key a machine has been seen to offer.
///
/// The fingerprint is what is compared; `key` is carried so the file can be read
/// and diffed by hand, which is half the reason it is JSON rather than a blob.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct KnownKey {
    /// `ssh-ed25519`, `rsa-sha2-512`, and so on. The half of the match that
    /// decides whether an unrecognised key is *new* or *changed*.
    pub algorithm: String,
    /// `SHA256:` and unpadded base64, as `ssh-keygen -l` prints it.
    pub fingerprint: String,
    /// The OpenSSH one-liner, for a human reading the file.
    #[serde(default)]
    pub key: String,
    #[serde(default)]
    pub added_at: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct KnownHosts {
    pub version: u32,
    /// Keyed by `host:port` exactly as [`endpoint`] spells it. A list rather
    /// than one key because a machine legitimately offers several -- an ed25519
    /// and an RSA -- and which one gets negotiated depends on both ends'
    /// preferences, so pinning only the first would make the second look like an
    /// attack.
    #[serde(default)]
    pub hosts: BTreeMap<String, Vec<KnownKey>>,
}

impl Default for KnownHosts {
    fn default() -> Self {
        KnownHosts {
            version: VERSION,
            hosts: BTreeMap::new(),
        }
    }
}

/// The key the machine actually presented, reduced to the three things this
/// module needs. Keeping `ssh_key` out of here is what lets the rules below be
/// tested without a server to connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PresentedKey {
    pub algorithm: String,
    pub fingerprint: String,
    pub openssh: String,
}

/// What the file has to say about a presented key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Seen before, byte for byte. Nothing to ask.
    Trusted,
    /// Nothing pinned for this machine, or nothing of this kind. A first
    /// connection, or a machine that has gained a key type it did not have.
    Unknown,
    /// A key of this kind is pinned and it is *not* this one. The loud case.
    Changed { known: Vec<String> },
}

/// How a machine is named in the file. Always spelled the same way, so the map
/// key is stable across however the user typed the host into the panel.
pub fn endpoint(host: &str, port: u16) -> String {
    format!("{}:{}", host.trim().to_lowercase(), port)
}

/// The rule, in full.
///
/// An exact fingerprint match is trust. Otherwise a pinned key *of the same
/// algorithm* means the machine's identity for that algorithm has changed, which
/// is the case worth shouting about. Anything else is simply new -- including a
/// machine that has grown an ed25519 key beside the RSA one it used to have,
/// which is an upgrade and not an attack.
pub fn judge(hosts: &KnownHosts, endpoint: &str, key: &PresentedKey) -> Verdict {
    let Some(pinned) = hosts.hosts.get(endpoint) else {
        return Verdict::Unknown;
    };

    if pinned
        .iter()
        .any(|known| known.fingerprint == key.fingerprint)
    {
        return Verdict::Trusted;
    }

    let clashing: Vec<String> = pinned
        .iter()
        .filter(|known| known.algorithm == key.algorithm)
        .map(|known| known.fingerprint.clone())
        .collect();

    if clashing.is_empty() {
        Verdict::Unknown
    } else {
        Verdict::Changed { known: clashing }
    }
}

/// Pin a key, replacing whatever was pinned for its algorithm.
///
/// Replacing rather than appending is what makes "trust" a usable answer to the
/// changed-key dialog: a machine that was rebuilt has one identity, not two, and
/// leaving the old fingerprint in would mean the next connection matched a key
/// that has already been shown to be presentable by someone else.
pub fn pin(hosts: &mut KnownHosts, endpoint: &str, key: &PresentedKey) {
    let entries = hosts.hosts.entry(endpoint.to_string()).or_default();
    entries.retain(|known| known.algorithm != key.algorithm);
    entries.push(KnownKey {
        algorithm: key.algorithm.clone(),
        fingerprint: key.fingerprint.clone(),
        key: key.openssh.clone(),
        added_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    });
}

pub fn path() -> PathBuf {
    config::data_dir().join("tshell.hosts.json")
}

#[derive(Debug)]
pub enum LoadError {
    /// Present, and not something this build should touch.
    FromTheFuture(u32),
    /// Present and unreadable: bad JSON, bad permissions, a truncated write.
    Unreadable(String),
}

impl LoadError {
    pub fn explain(&self) -> String {
        let path = path();
        match self {
            LoadError::FromTheFuture(found) => format!(
                "{} was written by a newer tshell (version {found}; this build reads {VERSION}). \
                 Refusing to open it, so that nothing pinned in it is lost.",
                path.display()
            ),
            LoadError::Unreadable(why) => format!(
                "{} could not be read: {why}. It has been left untouched -- every machine will \
                 be asked about again until it is fixed or removed.",
                path.display()
            ),
        }
    }
}

/// Read the pinned keys. A missing file is a first run, not a failure.
pub fn load() -> Result<KnownHosts, LoadError> {
    let path = path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(KnownHosts::default())
        }
        Err(error) => return Err(LoadError::Unreadable(error.to_string())),
    };

    // The version is read before the rest, so that a file from a newer build is
    // refused on the strength of the one field we are sure we understand.
    let probe: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    let found = probe
        .get("version")
        .and_then(|value| value.as_u64())
        .unwrap_or(0) as u32;
    if found > VERSION {
        return Err(LoadError::FromTheFuture(found));
    }

    let mut hosts: KnownHosts =
        serde_json::from_str(&text).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    hosts.version = VERSION;
    Ok(hosts)
}

/// Written owner-only: the file holds no secret, but it is what decides which
/// machines we will talk to without asking, and another account on the same box
/// has no business editing it.
pub fn save(hosts: &KnownHosts) -> Result<(), String> {
    let text = serde_json::to_string_pretty(hosts).map_err(|error| error.to_string())?;
    atomic::write_private(&path(), &text).map_err(|error| error.to_string())
}

// ------------------------------------------------------------------- gate ---

/// What the user said about a key they were shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Accept and pin it. The answer that makes the next connection silent.
    Trust,
    /// Accept for as long as this window is open, and write nothing down.
    Once,
    Reject,
}

impl Decision {
    fn parse(value: &str) -> Self {
        match value {
            "trust" => Decision::Trust,
            "once" => Decision::Once,
            // Anything else -- a typo, a future answer this build does not have,
            // a dismissed dialog -- is the answer that changes nothing.
            _ => Decision::Reject,
        }
    }
}

/// The one gate every connection passes through.
///
/// Process-global rather than Tauri state because what it guards is one file and
/// one window, and because [`crate::ssh::connect`] is reached from a dozen
/// commands and from inside a transfer job reconnecting on its own -- threading a
/// handle through all of those would put a parameter on every one of them to say
/// the same thing.
pub struct HostKeys {
    /// Set once the window exists. Absent means there is nobody to ask, and a
    /// question nobody can answer is a refusal -- see [`HostKeys::ask`].
    app: Mutex<Option<AppHandle>>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Decision>>>,
    seq: AtomicU64,
    /// The "just this once" answers: endpoint and fingerprint, never written
    /// down, gone when the process is.
    session: Mutex<HashSet<(String, String)>>,
    /// One question in flight per machine.
    ///
    /// Opening a terminal, a transfer panel and an assistant against the same
    /// new host is three connections racing, and without this the user is asked
    /// the same question three times and two of the answers are thrown away.
    /// Whoever arrives first asks; the rest wait, then find the answer already
    /// recorded and never see a dialog at all.
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

pub static HOST_KEYS: LazyLock<HostKeys> = LazyLock::new(HostKeys::new);

impl HostKeys {
    fn new() -> Self {
        HostKeys {
            app: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            seq: AtomicU64::new(1),
            session: Mutex::new(HashSet::new()),
            gates: Mutex::new(HashMap::new()),
        }
    }

    /// Called from `setup`, once there is a window to put a dialog in front of.
    pub fn install(&self, app: AppHandle) {
        *self.app.lock().unwrap() = Some(app);
    }

    fn accepted_this_run(&self, endpoint: &str, fingerprint: &str) -> bool {
        self.session
            .lock()
            .unwrap()
            .contains(&(endpoint.to_string(), fingerprint.to_string()))
    }

    fn accept_this_run(&self, endpoint: &str, fingerprint: &str) {
        self.session
            .lock()
            .unwrap()
            .insert((endpoint.to_string(), fingerprint.to_string()));
    }

    /// The whole decision, for one presented key. `false` drops the connection.
    pub async fn verify(&self, endpoint: &str, label: &str, key: PresentedKey) -> bool {
        let gate = {
            let mut gates = self.gates.lock().unwrap();
            Arc::clone(
                gates
                    .entry(endpoint.to_string())
                    .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
            )
        };
        let _asking = gate.lock().await;

        // Read behind the gate rather than before it: the connection that just
        // finished asking may have been asking about this very key.
        if self.accepted_this_run(endpoint, &key.fingerprint) {
            return true;
        }

        let (hosts, notice) = match load() {
            Ok(hosts) => (hosts, String::new()),
            // Unreadable is not "trust nothing and say nothing": it is "ask
            // about everything, and say why". `hosts` is empty, so every verdict
            // below comes out Unknown.
            Err(error) => (KnownHosts::default(), error.explain()),
        };

        let verdict = judge(&hosts, endpoint, &key);
        if verdict == Verdict::Trusted {
            return true;
        }

        match self.ask(endpoint, label, &key, &verdict, &notice).await {
            Decision::Reject => false,
            Decision::Once => {
                self.accept_this_run(endpoint, &key.fingerprint);
                true
            }
            Decision::Trust => {
                /*
                 * The file is re-read rather than the copy above reused. The
                 * question stood for as long as the user took to read it, and in
                 * that time another connection may have pinned a different
                 * machine, or the user may have edited the file by hand. Writing
                 * the stale copy back would undo whichever it was.
                 */
                let mut fresh = match load() {
                    Ok(hosts) => hosts,
                    // Still unreadable. Honour the answer for this run, but do
                    // not start a new file over the top of one we cannot read.
                    Err(_) => {
                        self.accept_this_run(endpoint, &key.fingerprint);
                        return true;
                    }
                };
                pin(&mut fresh, endpoint, &key);
                if let Err(error) = save(&fresh) {
                    // The connection was approved; only the remembering failed.
                    // Holding it for this run keeps the user from being asked
                    // again on every tab they open until they fix the disk.
                    eprintln!("[hosts] {} could not be written: {error}", path().display());
                    self.accept_this_run(endpoint, &key.fingerprint);
                }
                true
            }
        }
    }

    async fn ask(
        &self,
        endpoint: &str,
        label: &str,
        key: &PresentedKey,
        verdict: &Verdict,
        notice: &str,
    ) -> Decision {
        // Cloned out of the lock rather than read through it: a `std::sync`
        // guard cannot cross the await below, and the emit itself has no
        // business happening with the lock held.
        let app = {
            let held = self.app.lock().unwrap();
            held.clone()
        };
        let Some(app) = app else {
            return Decision::Reject;
        };

        let (status, known) = match verdict {
            Verdict::Changed { known } => ("changed", known.clone()),
            _ => ("unknown", Vec::new()),
        };

        let id = self.seq.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let posted = app.emit(
            ASK_EVENT,
            json!({
                "id": id,
                "endpoint": endpoint,
                "label": label,
                "status": status,
                "algorithm": key.algorithm,
                "fingerprint": key.fingerprint,
                "known": known,
                "notice": notice,
            }),
        );
        if posted.is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Decision::Reject;
        }

        match tokio::time::timeout(ANSWER_TIMEOUT, rx).await {
            Ok(Ok(decision)) => decision,
            // Timed out, or the sender was dropped because the window went away
            // mid-question. Either way nobody said yes.
            _ => {
                self.pending.lock().unwrap().remove(&id);
                Decision::Reject
            }
        }
    }

    /// The answer, arriving as its own command however long after the question.
    pub fn answer(&self, id: u64, choice: &str) {
        if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
            let _ = tx.send(Decision::parse(choice));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(algorithm: &str, fingerprint: &str) -> PresentedKey {
        PresentedKey {
            algorithm: algorithm.to_string(),
            fingerprint: fingerprint.to_string(),
            openssh: format!("{algorithm} AAAA{fingerprint}"),
        }
    }

    fn pinned(endpoint: &str, entries: &[(&str, &str)]) -> KnownHosts {
        let mut hosts = KnownHosts::default();
        for (algorithm, fingerprint) in entries {
            pin(&mut hosts, endpoint, &key(algorithm, fingerprint));
        }
        hosts
    }

    #[test]
    fn endpoint_is_spelled_one_way() {
        assert_eq!(endpoint("Example.COM", 22), "example.com:22");
        assert_eq!(endpoint("  10.0.0.1 ", 2222), "10.0.0.1:2222");
    }

    #[test]
    fn a_machine_nobody_has_met_is_unknown() {
        let hosts = KnownHosts::default();
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:one")),
            Verdict::Unknown
        );
    }

    #[test]
    fn the_same_key_again_is_silent() {
        let hosts = pinned("a:22", &[("ssh-ed25519", "SHA256:one")]);
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:one")),
            Verdict::Trusted
        );
    }

    #[test]
    fn a_different_key_of_the_same_kind_is_the_loud_case() {
        let hosts = pinned("a:22", &[("ssh-ed25519", "SHA256:one")]);
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:two")),
            Verdict::Changed {
                known: vec!["SHA256:one".to_string()]
            }
        );
    }

    /// A machine that gains an ed25519 key beside its RSA one has been upgraded,
    /// not swapped. Shouting about that would only teach the user to click
    /// through the shout that matters.
    #[test]
    fn a_new_algorithm_on_a_known_machine_is_only_new() {
        let hosts = pinned("a:22", &[("rsa-sha2-512", "SHA256:rsa")]);
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:ed")),
            Verdict::Unknown
        );
    }

    #[test]
    fn one_machine_may_hold_several_kinds_at_once() {
        let hosts = pinned(
            "a:22",
            &[("rsa-sha2-512", "SHA256:rsa"), ("ssh-ed25519", "SHA256:ed")],
        );
        assert_eq!(hosts.hosts["a:22"].len(), 2);
        assert_eq!(
            judge(&hosts, "a:22", &key("rsa-sha2-512", "SHA256:rsa")),
            Verdict::Trusted
        );
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:ed")),
            Verdict::Trusted
        );
    }

    /// Trusting a changed key has to leave one identity behind, not two: an old
    /// fingerprint left in the file is a key someone has already been shown able
    /// to present, and it would match silently for ever after.
    #[test]
    fn trusting_a_changed_key_replaces_the_old_one() {
        let mut hosts = pinned("a:22", &[("ssh-ed25519", "SHA256:one")]);
        pin(&mut hosts, "a:22", &key("ssh-ed25519", "SHA256:two"));

        assert_eq!(hosts.hosts["a:22"].len(), 1);
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:two")),
            Verdict::Trusted
        );
        assert_eq!(
            judge(&hosts, "a:22", &key("ssh-ed25519", "SHA256:one")),
            Verdict::Changed {
                known: vec!["SHA256:two".to_string()]
            }
        );
    }

    #[test]
    fn machines_do_not_borrow_each_others_keys() {
        let hosts = pinned("a:22", &[("ssh-ed25519", "SHA256:one")]);
        assert_eq!(
            judge(&hosts, "b:22", &key("ssh-ed25519", "SHA256:one")),
            Verdict::Unknown
        );
        // The port is part of the name: a forwarded 2222 is not the machine on 22.
        assert_eq!(
            judge(&hosts, "a:2222", &key("ssh-ed25519", "SHA256:one")),
            Verdict::Unknown
        );
    }

    #[test]
    fn the_file_survives_a_round_trip() {
        let hosts = pinned(
            "a:22",
            &[("ssh-ed25519", "SHA256:ed"), ("rsa-sha2-512", "SHA256:rsa")],
        );
        let text = serde_json::to_string_pretty(&hosts).unwrap();
        let back: KnownHosts = serde_json::from_str(&text).unwrap();
        assert_eq!(back.version, VERSION);
        assert_eq!(back.hosts["a:22"], hosts.hosts["a:22"]);
    }

    /// A file with no `hosts` key at all still loads: it is what an empty one
    /// looks like after a hand-edit, and refusing it would ask about every
    /// machine for no reason.
    #[test]
    fn a_bare_version_is_an_empty_file() {
        let back: KnownHosts = serde_json::from_str(r#"{"version":1}"#).unwrap();
        assert!(back.hosts.is_empty());
    }

    #[test]
    fn a_decision_that_is_not_a_word_we_know_is_a_refusal() {
        assert_eq!(Decision::parse("trust"), Decision::Trust);
        assert_eq!(Decision::parse("once"), Decision::Once);
        assert_eq!(Decision::parse("reject"), Decision::Reject);
        assert_eq!(Decision::parse(""), Decision::Reject);
        assert_eq!(Decision::parse("yes"), Decision::Reject);
    }
}
