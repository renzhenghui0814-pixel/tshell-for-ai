//! The procedures someone wrote for this setup.
//!
//! A skill is a directory under `skills/` holding a `SKILL.md` with a short front
//! matter block, plus whatever else it wants to carry -- reference notes, a
//! script to put on the machine. Only the descriptions are ever resident: the
//! manifest built here goes into every request, and a body is pulled in one step
//! at a time, by the model, when it decides the skill is relevant.
//!
//! That is the whole reason this exists next to memory rather than inside it.
//! Memory is a handful of one-line facts and every one of them is paid for on
//! every message, greetings included. A procedure runs to pages and is wanted on
//! perhaps one task in twenty, so it is described in a line and read on demand.
//!
//! Nothing here runs anything. A skill is text: what it costs is context, and the
//! only machine it can reach is the one the model reaches through its own actions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::ai::types::SkillOutcome;

/// Longer than any sane directory name and short enough to survive a filesystem.
const MAX_ID_LENGTH: usize = 64;

/// What a manifest line may spend on one skill, so nobody's essay crowds it out.
const MAX_DESCRIPTION_LENGTH: usize = 200;

/// The file a `skill` action reads when it names no other.
pub const MAIN_FILE: &str = "SKILL.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSummary {
    /// The directory name. What the model names in a `skill` action.
    pub id: String,
    /// Front matter `name`, or the id when it says nothing. Display only.
    pub name: String,
    /// Front matter `description`. Empty means this skill cannot be offered.
    pub description: String,
    /// Off in `settings.ai.skills.disabled`. Kept out of the manifest and refused.
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRead {
    pub outcome: SkillOutcome,
    /// The file as it was read, middle elided when it ran past the budget.
    pub content: Option<String>,
    /// The path actually read, relative to the skill directory.
    pub file: Option<String>,
    pub truncated: bool,
}

impl SkillRead {
    fn failed(outcome: SkillOutcome, file: Option<String>) -> Self {
        Self {
            outcome,
            content: None,
            file,
            truncated: false,
        }
    }
}

/// The id, reduced to something that cannot climb out of the skills directory.
///
/// Same treatment memory gives a server id, and for the same reason: this string
/// arrives from a model, so `../../tshell.config.json` is a real input and not a
/// hypothetical one. Anything outside a conservative set becomes an underscore.
fn safe_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_ID_LENGTH)
        .collect();
    cleaned.trim_start_matches('.').to_string()
}

/// A relative path inside one skill, or `None` when it is not one.
///
/// Stricter than the id because this one is allowed to contain slashes, which is
/// exactly what makes it worth checking twice: the string is filtered here and
/// the resolved path is compared against the directory afterwards. A symlink can
/// walk out of a directory without any `..` ever appearing in the text.
fn safe_relative(file: &str) -> Option<String> {
    let trimmed = file.trim().replace('\\', "/");
    if trimmed.is_empty() || trimmed.len() > 256 {
        return None;
    }
    if trimmed.starts_with('/') {
        return None;
    }
    // A drive letter, which is absolute on Windows and nonsense anywhere else.
    let bytes = trimmed.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return None;
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'))
    {
        return None;
    }
    let parts: Vec<&str> = trimmed
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() || parts.iter().any(|part| *part == "..") {
        return None;
    }
    Some(parts.join("/"))
}

/// The `key: value` lines between the opening and closing `---`.
///
/// Hand-rolled rather than a YAML dependency, because two keys are read and
/// everything else is ignored. A file with no front matter parses to nothing,
/// which is the same as a file whose front matter says nothing.
pub fn parse_front_matter(text: &str) -> HashMap<String, String> {
    let mut fields = HashMap::new();
    // A byte order mark ahead of the fence is what a Windows editor leaves.
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = body
        .strip_prefix("---\n")
        .or_else(|| body.strip_prefix("---\r\n"))
    else {
        return fields;
    };

    let mut block = None;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            block = Some(&rest[..at]);
            break;
        }
        at += line.len();
    }
    // A fence that never closes is not front matter, the same as having none.
    let Some(block) = block.or_else(|| (rest.trim_end() == "---").then_some("")) else {
        return fields;
    };

    for line in block.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty()
            || !key.starts_with(|c: char| c.is_ascii_alphabetic())
            || !key
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
        {
            continue;
        }
        // Quotes are what someone reaches for when a description holds a colon,
        // so they are taken off rather than carried into the prompt as literal
        // text.
        let mut value = value.trim();
        for quote in ['"', '\''] {
            if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
                value = &value[1..value.len() - 1];
                break;
            }
        }
        let value = value.trim();
        if !value.is_empty() {
            fields.insert(key.to_lowercase(), value.to_string());
        }
    }
    fields
}

pub struct SkillStore {
    dir: PathBuf,
}

impl SkillStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn root(&self) -> &Path {
        &self.dir
    }

    pub fn dir_for(&self, id: &str) -> PathBuf {
        self.dir.join(safe_id(id))
    }

    /// Every skill on disk, in name order, whether or not it can be offered.
    ///
    /// One without a description is still returned. It cannot go in the manifest
    /// -- the description is the only thing the model has to choose by -- but
    /// dropping it here as well would mean someone writes a skill, sees nothing
    /// happen, and has nowhere to find out why. The panel shows these greyed with
    /// the reason.
    ///
    /// Every failure is an empty list. A setup with no skills is an ordinary setup.
    pub fn list(&self, disabled: &[String]) -> Vec<SkillSummary> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let off: Vec<String> = disabled.iter().map(|id| safe_id(id)).collect();

        let mut skills = Vec::new();
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            let id = safe_id(&name);
            // A name that does not survive sanitising is a name the model could
            // never pronounce back, so it is not a skill that can be loaded.
            if id.is_empty() || id != name {
                continue;
            }

            let Ok(text) = std::fs::read_to_string(entry.path().join(MAIN_FILE)) else {
                continue;
            };
            let fields = parse_front_matter(&text);
            skills.push(SkillSummary {
                name: fields.get("name").cloned().unwrap_or_else(|| id.clone()),
                description: fields
                    .get("description")
                    .map(|text| text.chars().take(MAX_DESCRIPTION_LENGTH).collect())
                    .unwrap_or_default(),
                disabled: off.contains(&id),
                id,
            });
        }
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        skills
    }

    /// The resident part: one line per skill the model may load.
    ///
    /// Empty when there is nothing to offer, which is what keeps the whole feature
    /// -- this block and the action that goes with it -- out of the prompt of
    /// anyone who has never written a skill.
    pub fn manifest(&self, disabled: &[String]) -> Vec<String> {
        self.list(disabled)
            .into_iter()
            .filter(|skill| !skill.disabled && !skill.description.is_empty())
            .map(|skill| format!("- {}: {}", skill.id, skill.description))
            .collect()
    }

    /// One file out of one skill.
    ///
    /// `budget` is the characters to keep. The middle goes rather than the end,
    /// the way command output is elided, so the shape of a long document survives.
    pub fn read(
        &self,
        id: &str,
        file: Option<&str>,
        budget: usize,
        disabled: &[String],
    ) -> SkillRead {
        let skill_id = safe_id(id);
        if skill_id.is_empty() {
            return SkillRead::failed(SkillOutcome::Unknown, None);
        }

        let skills = self.list(disabled);
        let Some(found) = skills.iter().find(|skill| skill.id == skill_id) else {
            return SkillRead::failed(SkillOutcome::Unknown, None);
        };
        if found.disabled {
            return SkillRead::failed(SkillOutcome::Disabled, None);
        }

        let asked = file.unwrap_or(MAIN_FILE);
        let Some(relative) = safe_relative(asked) else {
            return SkillRead::failed(SkillOutcome::Missing, Some(asked.to_string()));
        };

        let root = self.dir_for(&skill_id);
        let target = root.join(&relative);
        // The second check, and the one that catches a symlink: the text was
        // clean, but where it landed is what matters.
        let resolved = std::fs::canonicalize(&target);
        let anchored = std::fs::canonicalize(&root);
        match (&resolved, &anchored) {
            (Ok(target), Ok(root)) if target == root || target.starts_with(root) => {}
            (Ok(_), Ok(_)) => {
                return SkillRead::failed(SkillOutcome::Missing, Some(relative));
            }
            // Nothing to resolve means nothing to read, which the open below says
            // in the same words.
            _ => return SkillRead::failed(SkillOutcome::Missing, Some(relative)),
        }

        let Ok(metadata) = std::fs::metadata(&target) else {
            return SkillRead::failed(SkillOutcome::Missing, Some(relative));
        };
        if !metadata.is_file() {
            return SkillRead::failed(SkillOutcome::Missing, Some(relative));
        }
        let Ok(content) = std::fs::read_to_string(&target) else {
            return SkillRead::failed(SkillOutcome::Missing, Some(relative));
        };
        if content.trim().is_empty() {
            return SkillRead::failed(SkillOutcome::Unreadable, Some(relative));
        }

        let length = content.chars().count();
        if budget > 0 && length > budget {
            let half = budget / 2;
            let dropped = length - budget;
            let head: String = content.chars().take(half).collect();
            let tail: String = content.chars().skip(length - half).collect();
            let content = format!(
                "{head}\n\n[... {dropped} characters omitted from the middle ...]\n\n{tail}"
            );
            return SkillRead {
                outcome: SkillOutcome::Ok,
                content: Some(content),
                file: Some(relative),
                truncated: true,
            };
        }
        SkillRead {
            outcome: SkillOutcome::Ok,
            content: Some(content),
            file: Some(relative),
            truncated: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("tshell-skill-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn skill(&self, id: &str, body: &str) {
            let dir = self.0.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(MAIN_FILE), body).unwrap();
        }
        fn store(&self) -> SkillStore {
            SkillStore::new(&self.0)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn front(name: &str, description: &str) -> String {
        format!("---\nname: {name}\ndescription: {description}\n---\n\nThe procedure.\n")
    }

    #[test]
    fn front_matter_reads_the_two_keys_and_ignores_the_rest() {
        let fields = parse_front_matter(&front("Deploy", "how releases go out"));
        assert_eq!(fields.get("name").unwrap(), "Deploy");
        assert_eq!(fields.get("description").unwrap(), "how releases go out");

        assert!(parse_front_matter("no front matter here").is_empty());
        assert!(parse_front_matter("---\nunclosed: yes\n").is_empty());
    }

    #[test]
    fn quotes_around_a_value_are_punctuation_rather_than_text() {
        let fields = parse_front_matter("---\ndescription: \"holds: a colon\"\n---\n");
        assert_eq!(fields.get("description").unwrap(), "holds: a colon");
        let fields = parse_front_matter("---\ndescription: 'single'\n---\n");
        assert_eq!(fields.get("description").unwrap(), "single");
    }

    #[test]
    fn skills_are_listed_in_name_order() {
        let temp = Temp::new("order");
        temp.skill("zulu", &front("Alpha", "first"));
        temp.skill("alpha", &front("Zulu", "last"));
        let listed = temp.store().list(&[]);
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].name, "Alpha");
        assert_eq!(listed[1].name, "Zulu");
    }

    #[test]
    fn a_skill_with_no_description_is_listed_but_not_offered() {
        let temp = Temp::new("nodesc");
        temp.skill("quiet", "---\nname: Quiet\n---\nbody\n");
        temp.skill("loud", &front("Loud", "says what it does"));
        let store = temp.store();
        assert_eq!(store.list(&[]).len(), 2);
        assert_eq!(store.manifest(&[]), vec!["- loud: says what it does"]);
    }

    #[test]
    fn a_disabled_skill_is_neither_offered_nor_readable() {
        let temp = Temp::new("disabled");
        temp.skill("deploy", &front("Deploy", "how releases go out"));
        let store = temp.store();
        let off = vec!["deploy".to_string()];
        assert!(store.manifest(&off).is_empty());
        assert_eq!(
            store.read("deploy", None, 0, &off).outcome,
            SkillOutcome::Disabled
        );
        assert!(store.list(&off)[0].disabled);
    }

    #[test]
    fn a_skill_that_was_never_written_is_unknown() {
        let temp = Temp::new("unknown");
        let store = temp.store();
        assert_eq!(
            store.read("nothing", None, 0, &[]).outcome,
            SkillOutcome::Unknown
        );
        assert_eq!(
            store.read("...", None, 0, &[]).outcome,
            SkillOutcome::Unknown
        );
        assert!(store.list(&[]).is_empty());
    }

    #[test]
    fn the_main_document_is_what_a_bare_load_gets() {
        let temp = Temp::new("main");
        temp.skill("deploy", &front("Deploy", "d"));
        let read = temp.store().read("deploy", None, 0, &[]);
        assert_eq!(read.outcome, SkillOutcome::Ok);
        assert_eq!(read.file.as_deref(), Some("SKILL.md"));
        assert!(read.content.unwrap().contains("The procedure."));
    }

    #[test]
    fn a_path_that_climbs_out_is_refused() {
        let temp = Temp::new("climb");
        temp.skill("deploy", &front("Deploy", "d"));
        std::fs::write(temp.0.join("secret.txt"), "not yours").unwrap();
        let store = temp.store();

        for attempt in [
            "../secret.txt",
            "/etc/passwd",
            "C:/Windows/x",
            "a/../../secret.txt",
        ] {
            assert_eq!(
                store.read("deploy", Some(attempt), 0, &[]).outcome,
                SkillOutcome::Missing,
                "{attempt}"
            );
        }
    }

    #[test]
    fn an_extra_file_inside_the_skill_is_readable() {
        let temp = Temp::new("extra");
        temp.skill("deploy", &front("Deploy", "d"));
        std::fs::create_dir_all(temp.0.join("deploy/notes")).unwrap();
        std::fs::write(temp.0.join("deploy/notes/rollback.md"), "roll it back").unwrap();

        let read = temp
            .store()
            .read("deploy", Some("notes/rollback.md"), 0, &[]);
        assert_eq!(read.outcome, SkillOutcome::Ok);
        assert_eq!(read.file.as_deref(), Some("notes/rollback.md"));
        assert_eq!(read.content.as_deref(), Some("roll it back"));
    }

    #[test]
    fn an_empty_file_is_unreadable_rather_than_missing() {
        let temp = Temp::new("blank");
        temp.skill("deploy", &front("Deploy", "d"));
        std::fs::write(temp.0.join("deploy/blank.md"), "   \n").unwrap();
        let read = temp.store().read("deploy", Some("blank.md"), 0, &[]);
        assert_eq!(read.outcome, SkillOutcome::Unreadable);
    }

    #[test]
    fn a_long_file_loses_its_middle_rather_than_its_end() {
        let temp = Temp::new("long");
        let body = format!("---\ndescription: d\n---\nSTART{}END", "x".repeat(500));
        temp.skill("deploy", &body);

        let read = temp.store().read("deploy", None, 100, &[]);
        assert_eq!(read.outcome, SkillOutcome::Ok);
        assert!(read.truncated);
        let content = read.content.unwrap();
        assert!(content.starts_with("---"));
        assert!(content.ends_with("END"));
        assert!(content.contains("characters omitted from the middle"));
    }

    #[test]
    fn a_directory_whose_name_would_be_rewritten_is_not_a_skill() {
        let temp = Temp::new("badname");
        temp.skill("has space", &front("Spaced", "d"));
        temp.skill(".hidden", &front("Hidden", "d"));
        assert!(temp.store().list(&[]).is_empty());
    }

    #[test]
    fn a_directory_with_no_main_document_is_not_a_skill() {
        let temp = Temp::new("nomain");
        std::fs::create_dir_all(temp.0.join("empty")).unwrap();
        assert!(temp.store().list(&[]).is_empty());
    }
}
