//! Which monospaced fonts this machine actually has.
//!
//! The settings page offers a font by name, and a name it offers had better be
//! one the machine can draw with -- a font that is not installed does not fail,
//! it silently falls back, and the user is left staring at the same glyphs
//! wondering what they got wrong. The web platform has no way to ask: the Local
//! Font Access API is Chromium-only, permission-gated and not reachable from a
//! custom protocol, so the question has to be answered on this side.
//!
//! Only the families are handed out, not the faces. A picker with "Consolas",
//! "Consolas Bold" and "Consolas Italic" in it is offering two things nobody
//! wants -- xterm picks the weight itself, per cell, from what the terminal is
//! being told to draw.
//!
//! Two lists come out of one scan: everything installed, for the window's own
//! text, and the monospaced subset, for the terminal and for code. One scan
//! because the scan is the expensive part -- a table read out of every font
//! file on the machine, the better part of a second on a developer's box -- and
//! running it twice to ask two questions about the same faces would double the
//! only cost this module has.

use std::collections::BTreeMap;
use std::sync::OnceLock;

/*
 * Scanned once and kept.
 *
 * `load_system_fonts` opens and reads a table out of every font file on the
 * machine, which on a developer's Windows box is a few hundred files and the
 * better part of a second. It is worth doing lazily -- the answer is only ever
 * needed by the settings page, which most sessions never open -- and it is
 * worth doing once, because the answer does not change while the window is up.
 */
static FAMILIES: OnceLock<Families> = OnceLock::new();

/// What one scan produces: every family, and the monospaced ones.
struct Families {
    all: Vec<String>,
    monospace: Vec<String>,
}

/// The monospaced families installed here, in the order a picker should list
/// them: alphabetical, ignoring case, so "consolas" and "Consolas" do not sit
/// at opposite ends.
pub fn monospace() -> &'static [String] {
    &FAMILIES.get_or_init(scan).monospace
}

/// Every family installed here, in the same order.
///
/// For the window's text, where a proportional face is the point -- offering
/// only the monospaced ones would be offering a terminal font for a settings
/// page.
pub fn all() -> &'static [String] {
    &FAMILIES.get_or_init(scan).all
}

fn scan() -> Families {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    /*
     * Keyed by the lower-cased name so a family that ships one file per weight
     * appears once, and so two files disagreeing about capitalisation do not
     * become two entries. The value keeps the original spelling, which is what
     * goes into a `font-family` and what the user expects to read.
     */
    let mut all: BTreeMap<String, String> = BTreeMap::new();
    let mut monospace: BTreeMap<String, String> = BTreeMap::new();

    for face in db.faces() {
        let Some(name) = family_name(face) else {
            continue;
        };
        if face.monospaced {
            monospace.entry(name.to_lowercase()).or_insert(name.clone());
        }
        all.entry(name.to_lowercase()).or_insert(name);
    }

    Families {
        all: all.into_values().collect(),
        monospace: monospace.into_values().collect(),
    }
}

/// The English name if the font offers one, else whatever it offers first.
///
/// A font carries its family name once per language it has been localised for,
/// and the localised name is not what a `font-family` matches on every
/// platform. Preferring English is preferring the name that works.
fn family_name(face: &fontdb::FaceInfo) -> Option<String> {
    face.families
        .iter()
        .find(|(_, language)| matches!(language, fontdb::Language::English_UnitedStates))
        .or_else(|| face.families.first())
        .map(|(name, _)| name.trim().to_string())
        .filter(|name| !name.is_empty())
}
