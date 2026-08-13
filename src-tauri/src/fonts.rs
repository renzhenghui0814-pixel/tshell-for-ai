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
static FAMILIES: OnceLock<Vec<String>> = OnceLock::new();

/// The monospaced families installed here, in the order a picker should list
/// them: alphabetical, ignoring case, so "consolas" and "Consolas" do not sit
/// at opposite ends.
pub fn monospace() -> &'static [String] {
    FAMILIES.get_or_init(scan)
}

fn scan() -> Vec<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();

    /*
     * Keyed by the lower-cased name so a family that ships one file per weight
     * appears once, and so two files disagreeing about capitalisation do not
     * become two entries. The value keeps the original spelling, which is what
     * goes into a `font-family` and what the user expects to read.
     */
    let mut families: BTreeMap<String, String> = BTreeMap::new();

    for face in db.faces() {
        if !face.monospaced {
            continue;
        }
        let Some(name) = family_name(face) else {
            continue;
        };
        families.entry(name.to_lowercase()).or_insert(name);
    }

    families.into_values().collect()
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
