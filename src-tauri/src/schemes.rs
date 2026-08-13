//! The terminal's appearance, as named schemes the user owns.
//!
//! A scheme is the whole of what a terminal looks like: the sixteen ANSI
//! colours, the four that are not ANSI (foreground, background, cursor,
//! selection), the font and its size. Three schemes ship with the product and
//! live in `ui/shared/palettes.js` -- they have to be there, because xterm is
//! handed a JavaScript object and cannot read a Rust one. What lives here is
//! everything the *user* made: schemes of their own, and edits to the three
//! built-in ones.
//!
//! So this file is deliberately not the whole model. It owns the file: the
//! version gate, the shape, what counts as a colour, and replacing it without
//! the chance of losing it. Merging a stored scheme onto a built-in is the
//! front end's, in the one place both are already in scope.
//!
//! It is its own file rather than a section of `tshell.config.json` because a
//! scheme is a thing you make and hand to someone, and twenty hex values per
//! scheme would otherwise be most of what the config file is.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::atomic;
use crate::config;

/// Bumped only when an older build would read this file and mean something else
/// by it. A file from a later version is refused, never guessed at.
pub const VERSION: u32 = 1;

/// Below this a terminal is unreadable, above it a row does not fit. Both ends
/// are generous -- the point is to refuse nonsense, not to have taste.
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 48.0;

/// A font stack is a CSS `font-family` value, and no real one is this long.
const MAX_FONT_LEN: usize = 200;

/// Long enough for any name someone would actually read off a picker.
const MAX_NAME_LEN: usize = 80;

// ------------------------------------------------------------------ colours ---

/*
 * Twenty named slots, every one optional.
 *
 * Optional because absent has a meaning that empty string does not: on an edit
 * to a built-in scheme it means "this one was not changed, keep what ships",
 * and on the four non-ANSI slots it means "follow the window's light/dark
 * theme", which is what the built-ins do and what kept the terminal from ever
 * disagreeing with the chrome around it.
 *
 * Named fields rather than a map: the twenty are a closed set, the JSON stays
 * in the order a person would read it in, and a key this build has never heard
 * of is dropped on the way in rather than written back out as though it meant
 * something.
 */
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Colors {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub black: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub red: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub green: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yellow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub magenta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cyan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub white: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_black: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_red: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_green: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_yellow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_blue: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_magenta: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_cyan: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bright_white: Option<String>,

    /*
     * The four that are not ANSI colours.
     *
     * These are why the prompt can be recoloured at all: a shell whose `PS1`
     * carries no escape sequence is drawn in the default foreground, which used
     * to be read out of the stylesheet and so was never anybody's choice.
     */
    #[serde(skip_serializing_if = "Option::is_none")]
    pub foreground: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
}

impl Colors {
    /// Every slot, in the order the settings page lists them.
    fn slots(&mut self) -> [&mut Option<String>; 20] {
        [
            &mut self.black,
            &mut self.red,
            &mut self.green,
            &mut self.yellow,
            &mut self.blue,
            &mut self.magenta,
            &mut self.cyan,
            &mut self.white,
            &mut self.bright_black,
            &mut self.bright_red,
            &mut self.bright_green,
            &mut self.bright_yellow,
            &mut self.bright_blue,
            &mut self.bright_magenta,
            &mut self.bright_cyan,
            &mut self.bright_white,
            &mut self.foreground,
            &mut self.background,
            &mut self.cursor,
            &mut self.selection,
        ]
    }

    /// Anything that is not a colour becomes absent, which is the one value
    /// every consumer already handles. Writing back a slot that says `"blue-ish"`
    /// would hand xterm something it silently paints black.
    fn normalize(&mut self) {
        for slot in self.slots() {
            *slot = slot.take().and_then(|value| hex(&value));
        }
    }

    fn is_empty(&mut self) -> bool {
        self.slots().iter().all(|slot| slot.is_none())
    }
}

/// `#RRGGBB`, upper case, or nothing.
///
/// Lenient about what arrives -- a missing `#`, three digits, either case, stray
/// whitespace -- and strict about what leaves, so the file has one spelling of
/// a colour and the settings page never has to compare two.
fn hex(raw: &str) -> Option<String> {
    let body = raw.trim().trim_start_matches('#');
    if !body.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let full = match body.len() {
        // `#abc` is the CSS shorthand for `#aabbcc`, and someone typing by hand
        // will use it.
        3 => body.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => body.to_string(),
        _ => return None,
    };
    Some(format!("#{}", full.to_ascii_uppercase()))
}

// ------------------------------------------------------------------ schemes ---

/*
 * One scheme, or one edit to a built-in one -- the same shape either way.
 *
 * A user's own scheme carries `colors`: one set, used whichever way the window
 * is themed, because a scheme that brings its own background is already either
 * a dark one or a light one and has no second half to offer.
 *
 * An edit to a built-in carries `dark` and `light` instead, because the three
 * built-ins do have two halves and follow the window. Recording the halves
 * separately is what keeps recolouring the dark half from quietly rewriting the
 * light one -- and makes "restore the original" nothing more than dropping this
 * record.
 */
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Scheme {
    /// Absent on an edit to a built-in: the map key is already its name.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Empty means "whatever this scheme is otherwise called". The front end
    /// picks the words -- a built-in has a name in two languages and this file
    /// should not be holding a third.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// A CSS `font-family` value. Empty follows the product's own stack.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub font: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font_size: Option<f32>,

    /// A user scheme's single set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colors: Option<Colors>,
    /// A built-in's two halves.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark: Option<Colors>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light: Option<Colors>,
}

impl Scheme {
    fn normalize(&mut self) {
        self.id = self.id.trim().to_string();
        self.name = truncate(self.name.trim(), MAX_NAME_LEN);
        self.font = truncate(self.font.trim(), MAX_FONT_LEN);
        self.font_size = self.font_size.and_then(|size| {
            if size.is_finite() && (MIN_FONT_SIZE..=MAX_FONT_SIZE).contains(&size) {
                // Half sizes are legible and whole ones are what people mean.
                Some((size * 2.0).round() / 2.0)
            } else {
                None
            }
        });

        for set in [&mut self.colors, &mut self.dark, &mut self.light] {
            if let Some(colors) = set {
                colors.normalize();
                if colors.is_empty() {
                    *set = None;
                }
            }
        }
    }

    /// Nothing in here is an answer to anything. An edit that says only "this
    /// built-in is called what it was already called" is not worth a record.
    fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.font.is_empty()
            && self.font_size.is_none()
            && self.colors.is_none()
            && self.dark.is_none()
            && self.light.is_none()
    }
}

/// The file.
/// Every field defaults, so a hand-written file may hold only the part its
/// author cared about -- which is half the reason this is a separate, readable
/// file rather than a section of the config.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct SchemesFile {
    pub version: u32,
    /// Which scheme the terminals paint with. Not resolved here: whether a name
    /// is real depends on the built-ins, which are the front end's.
    pub selected: String,
    /// The user's own, in the order the picker lists them.
    pub schemes: Vec<Scheme>,
    /// Edits to a built-in, keyed by the built-in's id.
    pub overrides: BTreeMap<String, Scheme>,
}

impl Default for SchemesFile {
    fn default() -> Self {
        SchemesFile {
            version: VERSION,
            selected: String::new(),
            schemes: Vec::new(),
            overrides: BTreeMap::new(),
        }
    }
}

impl SchemesFile {
    fn normalize(&mut self) {
        self.version = VERSION;
        self.selected = self.selected.trim().to_string();

        /*
         * Two schemes answering to one id is how editing one of them edits the
         * other. Ids are generated here rather than demanded of the front end
         * so that a hand-written file -- which is half the reason this is its
         * own readable file -- does not have to invent them.
         */
        let mut seen = std::collections::HashSet::new();
        for scheme in &mut self.schemes {
            scheme.normalize();
            if scheme.id.is_empty() || !seen.insert(scheme.id.clone()) {
                let mut fresh = config::make_id();
                while !seen.insert(fresh.clone()) {
                    fresh = config::make_id();
                }
                scheme.id = fresh;
            }
        }

        for scheme in self.overrides.values_mut() {
            scheme.normalize();
            // The key is the id. Carrying a second copy in the value only
            // creates the chance of the two disagreeing.
            scheme.id.clear();
        }
        self.overrides.retain(|_, scheme| !scheme.is_empty());
    }
}

fn truncate(value: &str, limit: usize) -> String {
    match value.char_indices().nth(limit) {
        Some((cut, _)) => value[..cut].to_string(),
        None => value.to_string(),
    }
}

// -------------------------------------------------------------------- disk ---

pub fn schemes_path() -> PathBuf {
    config::data_dir().join("tshell.schemes.json")
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
                 and until then the terminal uses the schemes that ship with tshell.",
                path.display()
            ),
        }
    }
}

/// Read the schemes, or say why not.
///
/// A missing file is a first run, not a failure: nobody has made a scheme yet,
/// and the three built-in ones are enough to open a terminal with. A file that
/// is present but broken *is* a failure, and one that leaves the file alone --
/// the caller stops offering to save rather than overwriting a file whose
/// contents it could not understand.
pub fn load() -> Result<SchemesFile, LoadError> {
    let raw = match std::fs::read_to_string(schemes_path()) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SchemesFile::default());
        }
        Err(error) => return Err(LoadError::Unreadable(error.to_string())),
    };
    parse(&raw)
}

/// The half of [`load`] with no filesystem in it, and so the half that is tested.
pub fn parse(raw: &str) -> Result<SchemesFile, LoadError> {
    // The version is read before the rest is interpreted: a later file may well
    // parse as this shape and mean something else by it.
    let probe: Value =
        serde_json::from_str(raw).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    let version = probe.get("version").and_then(Value::as_u64).unwrap_or(0) as u32;
    if version > VERSION {
        return Err(LoadError::FutureVersion(version));
    }

    let mut file: SchemesFile =
        serde_json::from_value(probe).map_err(|error| LoadError::Unreadable(error.to_string()))?;
    file.normalize();
    Ok(file)
}

pub fn save(file: &SchemesFile) -> Result<SchemesFile, String> {
    // Normalized before it is written, not after it is read, so that what the
    // caller gets back is exactly what the next load will produce.
    let mut file = file.clone();
    file.normalize();
    let body = serde_json::to_string_pretty(&file).map_err(|error| error.to_string())?;
    atomic::write(&schemes_path(), &format!("{body}\n")).map_err(|error| error.to_string())?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(raw: &str) -> SchemesFile {
        parse(raw).expect("should parse")
    }

    /// The point of the version field. A file from a later build is refused, not
    /// read as best it can be and written back with whatever it did not
    /// understand quietly dropped.
    #[test]
    fn a_later_version_is_refused() {
        let raw = r##"{"version": 2, "schemes": []}"##;
        assert!(matches!(parse(raw), Err(LoadError::FutureVersion(2))));
    }

    #[test]
    fn a_missing_version_reads_as_the_current_one() {
        let file = parse_ok(r##"{"schemes": []}"##);
        assert_eq!(file.version, VERSION);
    }

    /// One spelling of a colour in the file, whatever spelling arrived.
    #[test]
    fn colours_are_normalized_to_six_upper_case_digits() {
        let file = parse_ok(
            r##"{"version":1,"schemes":[{"id":"a","colors":{
                 "red":"#ff6b6b", "green":"0E8563", "blue":"#abc", "yellow":"  #F5B546  "
               }}]}"##,
        );
        let colors = file.schemes[0].colors.as_ref().unwrap();
        assert_eq!(colors.red.as_deref(), Some("#FF6B6B"));
        assert_eq!(colors.green.as_deref(), Some("#0E8563"));
        assert_eq!(colors.blue.as_deref(), Some("#AABBCC"));
        assert_eq!(colors.yellow.as_deref(), Some("#F5B546"));
    }

    /// Absent is the one value every consumer handles. A slot that is not a
    /// colour becomes absent rather than being handed to xterm, which paints
    /// anything it cannot parse black.
    #[test]
    fn a_slot_that_is_not_a_colour_becomes_absent() {
        let file = parse_ok(
            r##"{"version":1,"schemes":[{"id":"a","colors":{
                 "red":"blue-ish", "green":"#12345", "blue":"", "cyan":"#0C7C8C"
               }}]}"##,
        );
        let colors = file.schemes[0].colors.as_ref().unwrap();
        assert_eq!(colors.red, None);
        assert_eq!(colors.green, None);
        assert_eq!(colors.blue, None);
        assert_eq!(colors.cyan.as_deref(), Some("#0C7C8C"));
    }

    /// A key from a later build, or a typo, is dropped on the way in -- not
    /// written back out as though this build had understood it.
    #[test]
    fn unknown_colour_keys_are_dropped() {
        let file = parse_ok(
            r##"{"version":1,"schemes":[{"id":"a","colors":{"red":"#FF0000","puce":"#CC8899"}}]}"##,
        );
        let rendered = serde_json::to_string(&file).unwrap();
        assert!(rendered.contains("#FF0000"));
        assert!(!rendered.contains("puce"));
    }

    #[test]
    fn font_size_outside_the_legible_range_is_dropped() {
        let file = parse_ok(
            r##"{"version":1,"schemes":[
                 {"id":"a","fontSize":2},{"id":"b","fontSize":900},
                 {"id":"c","fontSize":13.7},{"id":"d","fontSize":14}
               ]}"##,
        );
        assert_eq!(file.schemes[0].font_size, None);
        assert_eq!(file.schemes[1].font_size, None);
        // Rounded to the nearest half, which is as fine as a font size gets.
        assert_eq!(file.schemes[2].font_size, Some(13.5));
        assert_eq!(file.schemes[3].font_size, Some(14.0));
    }

    /// Two schemes answering to one id is how editing one of them edits the
    /// other. A hand-written file that forgot to give one an id gets one.
    #[test]
    fn ids_are_made_unique() {
        let file = parse_ok(
            r##"{"version":1,"schemes":[{"id":"a"},{"id":"a"},{"name":"no id at all"}]}"##,
        );
        assert_eq!(file.schemes[0].id, "a");
        assert_ne!(file.schemes[1].id, "a");
        assert!(!file.schemes[1].id.is_empty());
        assert!(!file.schemes[2].id.is_empty());
        assert_ne!(file.schemes[1].id, file.schemes[2].id);
    }

    /// Editing the dark half of a built-in must not rewrite its light half.
    /// The halves are recorded separately for exactly this reason.
    #[test]
    fn an_edit_to_a_built_in_keeps_the_halves_apart() {
        let file = parse_ok(
            r##"{"version":1,"overrides":{"nebula":{"dark":{"red":"#AA0000"}}}}"##,
        );
        let edit = &file.overrides["nebula"];
        assert_eq!(edit.dark.as_ref().unwrap().red.as_deref(), Some("#AA0000"));
        assert!(edit.light.is_none());
        assert!(edit.colors.is_none());
    }

    /// "Restore the original" is nothing but dropping the record, so a record
    /// that no longer says anything should not survive a save.
    #[test]
    fn an_edit_that_changes_nothing_is_dropped() {
        let file = parse_ok(
            r##"{"version":1,"overrides":{
                 "nebula":{"dark":{"red":"not a colour"}},
                 "vscode":{"light":{"blue":"#0000FF"}}
               }}"##,
        );
        assert!(!file.overrides.contains_key("nebula"));
        assert!(file.overrides.contains_key("vscode"));
    }

    /// The id lives in the map key. A second copy in the value is only a chance
    /// for the two to disagree.
    #[test]
    fn an_edit_does_not_carry_its_own_id() {
        let file = parse_ok(
            r##"{"version":1,"overrides":{"nebula":{"id":"something-else","name":"Mine"}}}"##,
        );
        assert_eq!(file.overrides["nebula"].id, "");
        assert_eq!(file.overrides["nebula"].name, "Mine");
    }

    /// What comes back from a save is what the next load will produce -- the
    /// settings page redraws from the return value, and it should not be
    /// showing something the file does not say.
    #[test]
    fn a_normalized_file_survives_a_round_trip() {
        let once = parse_ok(
            r##"{"version":1,"selected":"mine","schemes":[
                 {"id":"mine","name":"Mine","font":"Consolas","fontSize":14,
                  "colors":{"red":"#ff0000","foreground":"#eeeeee"}}
               ],"overrides":{"nebula":{"dark":{"blue":"#123456"}}}}"##,
        );
        let rendered = serde_json::to_string(&once).unwrap();
        let twice = parse_ok(&rendered);
        assert_eq!(
            serde_json::to_string(&twice).unwrap(),
            rendered,
            "normalizing twice should change nothing"
        );
        assert_eq!(twice.selected, "mine");
        assert_eq!(twice.schemes[0].font, "Consolas");
    }

    /// A name is the user's words and the file should hold them, but not an
    /// unbounded number of them.
    #[test]
    fn a_name_is_trimmed_and_bounded() {
        let long = "x".repeat(200);
        let raw = format!(r##"{{"version":1,"schemes":[{{"id":"a","name":"  {long}  "}}]}}"##);
        let file = parse_ok(&raw);
        assert_eq!(file.schemes[0].name.chars().count(), MAX_NAME_LEN);
    }
}
