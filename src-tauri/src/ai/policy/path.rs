//! The red line for the file actions.
//!
//! `super::command` guards what may run; this guards where bytes may land.
//! Without it the file actions would be the one way around the risk scanner:
//! writing a payload to /dev/sda is the raw disk write that is already refused
//! when it is spelled as a command, so it has to be refused when it is spelled as
//! a path too.
//!
//! Everything this does not refuse is confirmed by the user, content and all, so
//! the list stays short on purpose: only the places where the bytes are not a
//! file at all, or where landing them breaks the machine before anyone can look.

use serde::{Deserialize, Serialize};

use super::risk::RiskReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PathVerdict {
    Refuse,
    Confirm,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathPolicyResult {
    pub verdict: PathVerdict,
    /// Populated for `refuse`, in the same i18n ids the command scanner uses.
    pub reasons: Vec<RiskReason>,
}

/// Whether `path` is `root` itself or something underneath it.
fn under(path: &str, root: &str) -> bool {
    path == root || path.starts_with(&format!("{root}/"))
}

/// Resolves `.` and `..` textually, which is all that is available here: the
/// target is a remote machine and this must answer offline. A relative path is
/// left alone and judged as-is, because the working directory is the user's and
/// unknowable.
pub fn normalize_path(path: &str) -> String {
    let trimmed = path.trim();
    if !trimmed.starts_with('/') {
        return trimmed.to_string();
    }
    let mut parts: Vec<&str> = Vec::new();
    for part in trimmed.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

pub fn classify_path(path: &str) -> PathPolicyResult {
    let normalized = normalize_path(path);
    let refuse = |reason| PathPolicyResult {
        verdict: PathVerdict::Refuse,
        reasons: vec![reason],
    };

    // Device nodes are not files: writing one addresses hardware.
    if under(&normalized, "/dev") {
        return refuse(RiskReason::WritesRawDevice);
    }
    // Kernel interfaces, where a write is a command to the kernel.
    if under(&normalized, "/proc") || under(&normalized, "/sys") {
        return refuse(RiskReason::WritesKernelInterface);
    }
    // The boot area, where a bad file is found at the next power cycle.
    if under(&normalized, "/boot") {
        return refuse(RiskReason::OverwritesSystemFile);
    }
    PathPolicyResult {
        verdict: PathVerdict::Confirm,
        reasons: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_and_the_kernel_are_refused() {
        assert_eq!(classify_path("/dev/sda").verdict, PathVerdict::Refuse);
        assert_eq!(classify_path("/dev").verdict, PathVerdict::Refuse);
        assert_eq!(
            classify_path("/proc/sys/kernel/x").verdict,
            PathVerdict::Refuse
        );
        assert_eq!(classify_path("/sys/class/x").verdict, PathVerdict::Refuse);
        assert_eq!(
            classify_path("/boot/grub/grub.cfg").verdict,
            PathVerdict::Refuse
        );
    }

    #[test]
    fn an_ordinary_file_is_only_confirmed() {
        assert_eq!(classify_path("/etc/hosts").verdict, PathVerdict::Confirm);
        assert_eq!(
            classify_path("/home/me/notes.txt").verdict,
            PathVerdict::Confirm
        );
        assert_eq!(classify_path("relative/file").verdict, PathVerdict::Confirm);
    }

    #[test]
    fn dot_dot_cannot_be_used_to_climb_back_in() {
        assert_eq!(
            classify_path("/tmp/../dev/sda").verdict,
            PathVerdict::Refuse
        );
        assert_eq!(
            classify_path("/dev/../etc/hosts").verdict,
            PathVerdict::Confirm
        );
    }

    #[test]
    fn a_name_that_merely_starts_the_same_is_not_underneath() {
        assert_eq!(classify_path("/devices/x").verdict, PathVerdict::Confirm);
        assert_eq!(classify_path("/bootstrap.sh").verdict, PathVerdict::Confirm);
    }

    #[test]
    fn paths_are_resolved_textually() {
        assert_eq!(normalize_path("  /a/./b/../c  "), "/a/c");
        assert_eq!(normalize_path("/.."), "/");
        assert_eq!(normalize_path("a/b"), "a/b");
    }
}
