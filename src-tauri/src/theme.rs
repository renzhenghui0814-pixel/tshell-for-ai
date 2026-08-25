//! The window's own palette, as the user's edits to it.
//!
//! `ui/shared/theme.css` holds the palette this product ships with: two halves,
//! dark and light, each a set of semantic tokens -- eight surfaces, the accent,
//! two fills that are not the accent, three levels of text, four meanings. What
//! lives here is everything the *user* changed about them, and nothing else.
//!
//! So the shape is edits, not a palette. A token the user never touched is
//! absent from this file, produces no CSS, and lands on whatever the stylesheet
//! says today -- which means a later build may retune the default palette and
//! everyone who never opened this panel comes along. A file that stored all
//! seventeen would freeze the first version of the palette into every install
//! that ever saved once.
//!
//! Eighteen tokens per half, not the whole stylesheet. The rest
//! -- the hovers, the soft fills, the focus ring, the ink that goes on an accent
//! fill -- are *derived* from these, in `ui/shared/palette.js`, because the
//! settings page has to show the result while a colour is still being dragged
//! and the shell has to write it into every frame. Deriving on this side would
//! mean a second copy of the default palette in Rust and a round trip per
//! pointer move.
//!
//! Its own file rather than a section of `tshell.config.json`, for the reason
//! the schemes are: thirty-six hex values is more than the config file is, and
//! a palette is a thing people hand to each other.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atomic;
use crate::config;

/// Bumped only when an older build would read this file and mean something else
/// by it. A file from a later version is refused, never guessed at.
pub const VERSION: u32 = 1;

/// A font stack is a CSS `font-family` value, and no real one is this long.
const MAX_FONT_LEN: usize = 200;

/*
 * The tokens a half may carry.
 *
 * A closed set, checked on the way in: a key this build has never heard of is
 * dropped rather than written back out as though it meant something, and --
 * more to the point -- rather than reaching the pages as a declaration nobody
 * wrote a rule for. The names are the stylesheet's own, minus the leading
 * dashes and in the camel case the front end reads them by.
 *
 * The order is the order the settings page lists them in: two columns of five
 * grounds/fills, then text, then the four meanings.
 */
const TOKENS: [&str; 17] = [
    "bgElev", "bgBase", "bgInput", "bgDialog", "bgTab", "bgMenu", "ac", "chatUser", "markdown",
    "progress", "tx", "txDim", "txFaint", "ok", "warn", "err", "info",
];

/*
 * What an older file called a token that has since been split.
 *
 * Dropping an unknown key is this file's rule and it is the right one, but
 * applied to `bgInset` in a file written by an earlier build it would throw
 * away a colour the user chose -- silently, on upgrade, which is the shape of
 * loss this module exists to refuse elsewhere. So the old name is read once and
 * given to its surviving heirs. Some former heirs are no longer editable, but
 * an older choice still reaches every live role it used to cover.
 *
 * `ui/shared/palette.js` carries the same table for the same reason the token
 * list is duplicated -- either side may be the first to read a given file --
 * and `scripts/palette-check.mjs` holds the two to each other.
 */
const SPLIT: [(&str, &[&str]); 3] = [
    ("bgInset", &["bgInput"]),
    ("bgRaise", &["bgTab"]),
    ("bgFloat", &["bgDialog", "bgMenu"]),
];

/// Whether a name is one of the seventeen. Public so the command layer can say
/// what it accepts without repeating the list.
pub fn is_token(name: &str) -> bool {
    TOKENS.contains(&name)
}

/*
 * One half of the palette: the tokens this user changed, keyed by name.
 *
 * A map and not a struct of seventeen `Option`s, which is what the schemes file
 * uses. The difference is that a scheme's twenty slots are a protocol with
 * xterm -- every one of them means something specific to a consumer that is not
 * this program -- while these seventeen are only ever handed back to the
 * stylesheet that named them. A map keeps this file, the front end's edit map
 * and the settings page's controls all reading the same keys, and a `BTreeMap`
 * keeps the JSON in one order so that saving twice does not rewrite the file.
 */
pub type Half = BTreeMap<String, String>;

fn normalize_half(half: &mut Half) {
    // Before the closed set drops it: a name this build split is still a
    // choice the user made, and it is worth two values rather than none.
    for (was, heirs) in SPLIT {
        let Some(value) = half.get(was).cloned() else {
            continue;
        };
        for heir in heirs {
            half.entry(heir.to_string())
                .or_insert_with(|| value.clone());
        }
    }
    half.retain(|name, _| is_token(name));
    for value in half.values_mut() {
        if let Some(clean) = hex(value) {
            *value = clean;
        }
    }
    // A value that is not a colour becomes no value at all. Written back it
    // would reach the pages as `--ac: blue-ish`, which no rule can use and
    // which the settings page would then offer as though it were the choice.
    half.retain(|_, value| hex(value).is_some());
}

/// `#RRGGBB`, upper case, or nothing.
///
/// Lenient about what arrives -- a missing `#`, three digits, either case, stray
/// whitespace -- and strict about what leaves, so the file has one spelling of
/// a colour and the settings page never has to compare two. The same rule the
/// schemes file applies, for the same reason; deliberately not shared with it,
/// because the two files version independently and a change to what a terminal
/// colour may be is not a change to what a window colour may be.
fn hex(raw: &str) -> Option<String> {
    let body = raw.trim().trim_start_matches('#');
    if body.is_empty() || !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full = match body.len() {
        3 => body.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => body.to_string(),
        _ => return None,
    };
    Some(format!("#{}", full.to_ascii_uppercase()))
}

// --------------------------------------------------------------------- file ---

/*
 * The fonts sit outside the two halves on purpose.
 *
 * Which typeface the window is set in is not a property of being dark or light
 * -- nobody wants one face by day and another by night -- and putting them in
 * the halves would mean setting the same name twice and having it drift once.
 *
 * The terminal's font is *not* here. It belongs to a scheme, where it can
 * differ per scheme, and a terminal that ignored its scheme's font because the
 * window had one would be the surprise.
 */
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeFile {
    pub version: u32,
    /// The window's text. Empty means the stylesheet's own stack.
    pub font: String,
    /// Code blocks, commands, previews. Empty means the stylesheet's own stack.
    pub font_mono: String,
    pub dark: Half,
    pub light: Half,
}

impl Default for ThemeFile {
    fn default() -> Self {
        ThemeFile {
            version: VERSION,
            font: String::new(),
            font_mono: String::new(),
            dark: Half::new(),
            light: Half::new(),
        }
    }
}

impl ThemeFile {
    fn normalize(&mut self) {
        self.version = VERSION;
        self.font = truncate(self.font.trim(), MAX_FONT_LEN);
        self.font_mono = truncate(self.font_mono.trim(), MAX_FONT_LEN);
        normalize_half(&mut self.dark);
        normalize_half(&mut self.light);
    }
}

fn truncate(value: &str, limit: usize) -> String {
    match value.char_indices().nth(limit) {
        Some((cut, _)) => value[..cut].to_string(),
        None => value.to_string(),
    }
}

// --------------------------------------------------------------------- disk ---

pub fn theme_path() -> PathBuf {
    config::data_dir().join("tshell.theme.json")
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
                "{} could not be read: {why}. It has been left untouched -- fix or remove it, \
                 and until then the window wears the palette that ships with tshell.",
                path.display()
            ),
        }
    }
}

/// Read the palette edits, or say why not.
///
/// A missing file is a first run: nobody has changed a colour, and the
/// stylesheet is a complete palette on its own. A file that is present but
/// broken is a failure that leaves the file alone -- the caller stops offering
/// to save rather than overwriting a file it could not understand, which is the
/// same rule the config and the schemes follow and for the same reason: a
/// stray comma must not cost somebody the palette they built.
pub fn load() -> Result<ThemeFile, LoadError> {
    let raw = match std::fs::read_to_string(theme_path()) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ThemeFile::default());
        }
        Err(error) => return Err(LoadError::Unreadable(error.to_string())),
    };
    parse(&raw)
}

/// The half of [`load`] with no filesystem in it, and so the half that is tested.
pub fn parse(raw: &str) -> Result<ThemeFile, LoadError> {
    // The version is read before the rest is interpreted: a later file may well
    // parse as this shape and mean something else by it.
    let probe: Value =
        serde_json::from_str(raw).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    let version = probe.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version > VERSION {
        return Err(LoadError::FutureVersion(version));
    }

    let mut file: ThemeFile =
        serde_json::from_value(probe).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    file.normalize();
    Ok(file)
}

pub fn save(file: &ThemeFile) -> Result<ThemeFile, String> {
    // Normalized before it is written, not after it is read, so that what the
    // caller gets back is exactly what the next load will produce.
    let mut file = file.clone();
    file.normalize();
    let body = serde_json::to_string_pretty(&file).map_err(|error| error.to_string())?;
    atomic::write(&theme_path(), &format!("{body}\n")).map_err(|error| error.to_string())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(raw: &str) -> ThemeFile {
        parse(raw).expect("should parse")
    }

    /*
     * A token this build split, arriving under the name an older build wrote.
     *
     * The closed set drops unknown keys, and that rule applied to `bgInset`
     * without the migration means an upgrade silently discards a colour the
     * user chose -- the same class of loss the version gate and the
     * "parse failure does not reset the file" rule exist to refuse. Both
     * heirs get it, because the old name meant both of them.
     */
    #[test]
    fn a_split_token_reaches_each_surviving_heir() {
        let raw = r##"{"version": 1, "dark": {"bgInset": "#111111", "bgRaise": "#222", "bgFloat": "#333333"}}"##;
        let file = parse_ok(raw);
        let dark = &file.dark;
        assert_eq!(dark.get("bgInput").map(String::as_str), Some("#111111"));
        assert_eq!(dark.get("bgTab").map(String::as_str), Some("#222222"));
        assert_eq!(dark.get("bgDialog").map(String::as_str), Some("#333333"));
        assert_eq!(dark.get("bgMenu").map(String::as_str), Some("#333333"));
        // The name it arrived under is not a token and does not survive.
        assert!(!dark.contains_key("bgInset"));
        assert!(!dark.contains_key("bgRaise"));
        assert!(!dark.contains_key("bgFloat"));
    }

    /// A file holding both spellings keeps the new one. It exists as soon as
    /// anyone saves on this build and then opens the file with an older one.
    #[test]
    fn the_new_name_wins_over_the_one_it_replaced() {
        let raw = r##"{"version": 1, "dark": {"bgInset": "#111111", "bgInput": "#ABCDEF"}}"##;
        let dark = parse_ok(raw).dark;
        assert_eq!(dark.get("bgInput").map(String::as_str), Some("#ABCDEF"));
    }

    /// Deleted outright rather than split: they named nothing any rule drew.
    #[test]
    fn a_deleted_token_is_dropped() {
        let raw = r##"{"version": 1, "dark": {"ai": "#F97316", "bgRaiseHi": "#444444", "bgCode": "#111111", "bgCard": "#222222", "ac": "#00FF00"}}"##;
        let dark = parse_ok(raw).dark;
        assert!(!dark.contains_key("ai"));
        assert!(!dark.contains_key("bgRaiseHi"));
        assert!(!dark.contains_key("bgCode"));
        assert!(!dark.contains_key("bgCard"));
        assert_eq!(dark.get("ac").map(String::as_str), Some("#00FF00"));
    }

    /// The point of the version field. A file from a later build is refused, not
    /// read as best it can be and written back with whatever it did not
    /// understand quietly dropped.
    #[test]
    fn a_later_version_is_refused() {
        let raw = r##"{"version": 2, "dark": {}}"##;
        assert!(matches!(parse(raw), Err(LoadError::FutureVersion(2))));
    }

    #[test]
    fn a_missing_version_reads_as_the_current_one() {
        let file = parse_ok(r##"{"dark": {}}"##);
        assert_eq!(file, ThemeFile::default());
    }

    /// One spelling of a colour in the file, whatever spelling arrived, so that
    /// the settings page never has to compare two.
    #[test]
    fn colours_are_normalized() {
        let file = parse_ok(r##"{"version": 1, "dark": {"ac": " #6e7cf7 ", "tx": "abc"}}"##);
        assert_eq!(file.dark.get("ac").unwrap(), "#6E7CF7");
        assert_eq!(file.dark.get("tx").unwrap(), "#AABBCC");
    }

    /// A value that is not a colour is dropped rather than written back. It
    /// would reach the pages as a declaration no rule can use, and the settings
    /// page would then offer it back as though it were the user's choice.
    #[test]
    fn a_value_that_is_not_a_colour_is_dropped() {
        let file = parse_ok(r##"{"version": 1, "light": {"ac": "blue-ish", "ok": "#0D7E5C"}}"##);
        assert!(!file.light.contains_key("ac"));
        assert_eq!(file.light.get("ok").unwrap(), "#0D7E5C");
    }

    /// The set is closed. A name this build does not know is not a colour it can
    /// place, and writing it back would make it look supported.
    #[test]
    fn an_unknown_token_is_dropped() {
        let file = parse_ok(r##"{"version": 1, "dark": {"brand": "#F97316", "ac": "#6E7CF7"}}"##);
        assert!(!file.dark.contains_key("brand"));
        assert!(file.dark.contains_key("ac"));
    }

    /// Absent means "not changed", which is what lets a later build retune the
    /// default palette for everyone who never opened the panel. A half that
    /// records only what was touched is the whole mechanism.
    #[test]
    fn only_what_was_changed_is_kept() {
        let file = parse_ok(r##"{"version": 1, "dark": {"ac": "#112233"}}"##);
        assert_eq!(file.dark.len(), 1);
        assert!(file.light.is_empty());
    }

    #[test]
    fn a_font_is_trimmed_and_bounded() {
        let long = "x".repeat(MAX_FONT_LEN + 40);
        let raw = format!(r##"{{"version": 1, "font": "  Inter  ", "fontMono": "{long}"}}"##);
        let file = parse_ok(&raw);
        assert_eq!(file.font, "Inter");
        assert_eq!(file.font_mono.chars().count(), MAX_FONT_LEN);
    }

    /// What `save` hands back must be what the next `load` produces, or the
    /// settings page redraws from something the file does not say.
    #[test]
    fn saving_is_idempotent_through_a_reload() {
        let mut file = ThemeFile::default();
        file.dark.insert("ac".into(), "6e7cf7".into());
        file.normalize();
        let body = serde_json::to_string(&file).unwrap();
        assert_eq!(parse_ok(&body), file);
    }
}
