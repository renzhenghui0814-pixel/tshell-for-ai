//! The string table, read from the same file the front end reads.
//!
//! `i18n.ts` was 699 lines of pure lookup, and it was already exported once, to
//! `ui/vendor/strings.json`, so that the pages could keep calling `t()` without
//! a host to ask. Porting those 341 keys into Rust source by hand would create a
//! second copy of them that has to be kept in step with the first, and the only
//! thing that would ever notice the two drifting apart is a user reading a
//! sentence in the wrong language.
//!
//! So there is one copy. The JSON is embedded at compile time -- it is already
//! shipped inside the binary as part of `ui/`, so this costs nothing at runtime
//! and nothing in the package -- and both sides look up the same strings.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::config::Language;

const TABLE: &str = include_str!("../../ui/vendor/strings.json");

type Tables = HashMap<String, HashMap<String, String>>;

fn tables() -> &'static Tables {
    static PARSED: OnceLock<Tables> = OnceLock::new();
    PARSED.get_or_init(|| {
        serde_json::from_str(TABLE).expect("ui/vendor/strings.json is not a table of tables")
    })
}

/// The string `key` names, in `language`.
///
/// A missing key returns the key itself, which is what the pages do too: a
/// screen showing `deleteGroupConfirm` is ugly, but it is legible and it points
/// straight at what is missing, which an empty string does not.
pub fn t(language: Language, key: &str) -> String {
    tables()
        .get(language.tag())
        .and_then(|table| table.get(key))
        .cloned()
        .unwrap_or_else(|| key.to_string())
}
