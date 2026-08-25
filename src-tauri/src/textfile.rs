//! A whole text file, for the editor pane: read it, and write it back.
//!
//! Separate from `preview.rs` on purpose, even though both read text through
//! [`crate::transfer`]. A preview reads a window at an offset and never has to
//! know what the rest of the file says; an editor holds the whole thing, because
//! saving writes the whole thing. Those are different reads with different
//! limits, and folding them into one call would mean a function whose answer is
//! right for one caller and dangerous for the other.
//!
//! What this module does NOT do is read a file it cannot edit. A file over the
//! limit, or one that is not text, comes back with an empty body and the reason
//! -- and the page then streams it through `preview.rs`, which has read files of
//! any size for as long as the panel has existed. One large-file path, already
//! proven, rather than a second one written here to do the same job.

use encoding_rs::Encoding;
use serde::{Deserialize, Serialize};

use crate::preview;
use crate::ssh;
use crate::transfer::{self, Transfers};

/// The most a file can be and still be opened for editing.
///
/// It is held whole in the page, highlighted line by line, and written whole on
/// every save. Two megabytes of source is an enormous file; two megabytes of log
/// is a Tuesday, and that is what the read-only path is for.
pub const EDIT_LIMIT: u64 = 2 * 1024 * 1024;

/// How much of an oversized file is read to decide whether it is text at all.
const SNIFF: usize = 4096;

/// What a file looked like when it was opened, and what it must still look like
/// when it is saved.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Stamp {
    pub size: u64,
    /// Seconds since the epoch, or zero when the side cannot report one.
    pub modified: u64,
}

impl From<transfer::Stat> for Stamp {
    fn from(stat: transfer::Stat) -> Self {
        Stamp {
            size: stat.size,
            modified: stat.modified,
        }
    }
}

/// Why a file opened read-only, or empty when it did not.
///
/// Binary is decided first because it is the more specific fact: a 40MB
/// executable is both, and "this is not text" is the half worth saying.
pub fn verdict(size: u64, binary: bool) -> &'static str {
    if binary {
        "binary"
    } else if size > EDIT_LIMIT {
        "tooBig"
    } else {
        ""
    }
}

/// Whether the file moved under the editor between opening it and saving it.
///
/// Size settles it when it can. The clock is consulted only when both readings
/// have one: a side that cannot report a modification time reports zero, and
/// treating zero as a time would make every save on such a server a conflict --
/// which teaches the user to press "overwrite" without reading it, the one habit
/// the dialog exists to prevent.
pub fn changed(base: &Stamp, now: &Stamp) -> bool {
    if base.size != now.size {
        return true;
    }
    if base.modified == 0 || now.modified == 0 {
        return false;
    }
    base.modified != now.modified
}

/// The text as bytes for the file, or `None` when the encoding cannot hold it.
///
/// `encoding_rs` substitutes what it cannot represent rather than refusing, so
/// a save that ignored `had_errors` would replace a character with a question
/// mark and report success. Losing a character is not something to find out
/// about by reading the file back afterwards.
///
/// The verdict comes from the same pass that produces the bytes, not from a
/// check before it. Asking twice would mean encoding the whole file twice to
/// learn something the first pass already knew.
pub fn encode(text: &str, encoding: &'static Encoding) -> Option<Vec<u8>> {
    let (bytes, _, had_errors) = encoding.encode(text);
    if had_errors {
        return None;
    }
    Some(bytes.into_owned())
}

/// A file, whole, or the reason it is being shown rather than edited.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub content: String,
    pub stamp: Stamp,
    /// Empty, `binary` or `tooBig`. The page shows the file read-only for either
    /// of the last two, streaming it through the preview commands.
    pub read_only: &'static str,
    pub language: &'static str,
}

pub async fn read(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
    label: &str,
) -> Result<TextFile, String> {
    let stamp: Stamp = transfer::stat(transfers, pane, side, target, path)
        .await?
        .into();
    let language = preview::language(path);

    /*
     * Over the limit, so the body is never read here -- only enough of it to
     * answer the one question the page still needs answered, which is whether to
     * show it as text or say it is not text.
     */
    if stamp.size > EDIT_LIMIT {
        let head = transfer::read_bytes(transfers, pane, side, target, path, 0, SNIFF).await?;
        return Ok(TextFile {
            content: String::new(),
            stamp,
            read_only: verdict(stamp.size, preview::looks_binary(&head)),
            language,
        });
    }

    let bytes =
        transfer::read_bytes(transfers, pane, side, target, path, 0, stamp.size as usize).await?;
    if preview::looks_binary(&bytes) {
        return Ok(TextFile {
            content: String::new(),
            stamp,
            read_only: "binary",
            language,
        });
    }

    /*
     * One decode over the whole file, which is the thing `preview.rs` cannot do
     * and the reason its known defect -- a multi-byte character split across a
     * chunk boundary -- cannot happen here. There are no boundaries.
     */
    let (text, _, _) = preview::encoding_of(label).decode(&bytes);
    Ok(TextFile {
        content: text.into_owned(),
        stamp,
        read_only: "",
        language,
    })
}

/// What came back from an attempt to save.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// True when nothing was written because the file had moved. The page asks,
    /// and asks again with `force`.
    pub conflict: bool,
    /// What is on disk now: the new stamp after a write, the other program's
    /// stamp after a conflict. Either way it is what the next save compares to.
    pub stamp: Stamp,
}

#[allow(clippy::too_many_arguments)]
pub async fn save(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
    label: &str,
    content: &str,
    base: Stamp,
    force: bool,
) -> Result<Saved, String> {
    let encoding = preview::encoding_of(label);
    let bytes = encode(content, encoding).ok_or("editorEncodingLost")?;

    /*
     * A file that will not stat has been deleted or renamed since it was opened.
     * That is a change like any other -- the default stamp differs from any real
     * one -- so it raises the same question instead of a separate error, and
     * answering "overwrite" writes the file back where it was.
     */
    let now: Stamp = transfer::stat(transfers, pane, side, target, path)
        .await
        .map(Stamp::from)
        .unwrap_or_default();

    if !force && changed(&base, &now) {
        return Ok(Saved {
            conflict: true,
            stamp: now,
        });
    }

    transfer::write_bytes(transfers, pane, side, target, path, &bytes).await?;

    /*
     * Read back rather than assumed. The size is knowable from here, but the
     * modification time is the server clock, and a stamp built from this
     * machine would make the very next save look like someone else edit.
     */
    let stamp: Stamp = transfer::stat(transfers, pane, side, target, path)
        .await
        .map(Stamp::from)
        .unwrap_or_default();
    Ok(Saved {
        conflict: false,
        stamp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two reasons a file opens read-only, and their order.
    ///
    /// Binary is checked first because it is the more specific fact: a 40MB
    /// executable is both, and "this is not text" is the useful half of that.
    #[test]
    fn a_file_is_editable_unless_it_is_binary_or_too_big() {
        assert_eq!(verdict(1024, false), "");
        assert_eq!(verdict(EDIT_LIMIT, false), "");
        assert_eq!(verdict(EDIT_LIMIT + 1, false), "tooBig");
        assert_eq!(verdict(10, true), "binary");
        assert_eq!(verdict(EDIT_LIMIT + 1, true), "binary");
    }

    /// A file that grew or shrank changed, whatever the clock says.
    #[test]
    fn a_different_size_is_a_change() {
        assert!(changed(
            &Stamp {
                size: 10,
                modified: 5
            },
            &Stamp {
                size: 11,
                modified: 5
            }
        ));
        assert!(!changed(
            &Stamp {
                size: 10,
                modified: 5
            },
            &Stamp {
                size: 10,
                modified: 5
            }
        ));
    }

    /// An edit that replaces one character with another leaves the size alone,
    /// so the clock is the only thing that can report it.
    #[test]
    fn a_different_time_at_the_same_size_is_a_change() {
        assert!(changed(
            &Stamp {
                size: 10,
                modified: 5
            },
            &Stamp {
                size: 10,
                modified: 6
            }
        ));
    }

    /// A side that cannot report a modification time reports zero, and zero is
    /// not a time. Comparing against it would make every save on such a server a
    /// conflict, which trains the user to press overwrite without reading -- the
    /// one habit this dialog exists to avoid.
    #[test]
    fn an_unknown_time_falls_back_to_the_size() {
        assert!(!changed(
            &Stamp {
                size: 10,
                modified: 0
            },
            &Stamp {
                size: 10,
                modified: 9
            }
        ));
        assert!(!changed(
            &Stamp {
                size: 10,
                modified: 9
            },
            &Stamp {
                size: 10,
                modified: 0
            }
        ));
        assert!(changed(
            &Stamp {
                size: 10,
                modified: 0
            },
            &Stamp {
                size: 12,
                modified: 0
            }
        ));
    }

    /// What was decoded to show has to encode back to the same bytes, or every
    /// save on a GB18030 file would quietly rewrite every character in it.
    #[test]
    fn text_survives_the_round_trip_in_both_encodings() {
        for label in ["utf8", "gb2312"] {
            let encoding = preview::encoding_of(label);
            let bytes = encode("行情 quote\r\n第二行\n", encoding).expect("encodable");
            let (text, _, _) = encoding.decode(&bytes);
            assert_eq!(text, "行情 quote\r\n第二行\n", "{label}");
        }
    }

    /// A character the encoding cannot hold does not silently become a question
    /// mark. There is exactly one of them -- GB18030 is a Unicode transformation
    /// format and reaches every other code point through its four-byte
    /// sequences -- and the assumption that there were many is what made an
    /// earlier version of this check a second pass over the whole file.
    #[test]
    fn a_character_the_encoding_cannot_hold_is_refused() {
        let gb = preview::encoding_of("gb2312");
        assert!(encode("行情 quote", gb).is_some());
        assert!(
            encode("\u{1F600}", gb).is_some(),
            "an emoji goes through GB18030"
        );
        // The one code point the GB18030 encoder refuses: a private-use
        // character the standard deliberately leaves out.
        assert!(encode("\u{E5E5}", gb).is_none());
        assert!(encode("\u{E5E5}", preview::encoding_of("utf8")).is_some());
    }
}
