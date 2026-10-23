//! Reading a file for the preview panel: plain text in slices, and DBF as a table.
//!
//! Both work against either pane, because [`crate::transfer::read_bytes`] does.
//! That is the whole reason the panel behaves the same for a file on this
//! machine and one three hops away -- there is no remote preview and local
//! preview, only a preview.
//!
//! Nothing here holds a file open between calls. Each chunk is a fresh read at
//! an offset the page hands back, which costs an open per chunk and buys the
//! property that a preview cannot pin a handle on a server after the tab that
//! asked for it has gone.

use encoding_rs::{Encoding, GB18030, UTF_8};
use serde::Serialize;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;

use crate::ssh;
use crate::transfer::{self, Transfers};

/// Half a megabyte at a time, as the extension read.
const TEXT_CHUNK: usize = 512 * 1024;
/// Rows per request. Enough to fill a screen several times over.
const DBF_ROWS: u32 = 300;

/// The two encodings the panel offers. `gb2312` is a label, not a claim: it is
/// decoded as GB18030, which is a superset and reads everything GB2312 does.
pub fn encoding_of(label: &str) -> &'static Encoding {
    match label.to_lowercase().as_str() {
        "gb2312" | "gbk" | "gb18030" => GB18030,
        _ => UTF_8,
    }
}

fn decode(bytes: &[u8], encoding: &'static Encoding) -> String {
    /*
     * One-shot per chunk, with no decoder carried between them -- which is what
     * the extension did, and which has the same consequence: a multi-byte
     * character straddling a 512KB boundary becomes one replacement character.
     *
     * Keeping decoder state is not possible here without also keeping the
     * chunks in order, and the page is free to ask for any offset it likes.
     * Fixing it properly means overlapping the reads, which is worth doing only
     * if anyone ever notices.
     */
    let (text, _, _) = encoding.decode(bytes);
    text.into_owned()
}

/// What the preview panel calls this file, which decides how it is highlighted.
pub fn language(path: &str) -> &'static str {
    let extension = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("")
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_lowercase())
        .unwrap_or_default();

    match extension.as_str() {
        "dbf" => "dbf",
        "xml" => "xml",
        "html" | "htm" => "html",
        "ini" | "conf" | "cfg" | "properties" | "env" => "ini",
        "csv" => "csv",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "java" => "java",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "css" => "css",
        "py" => "python",
        "sh" => "shell",
        _ => "text",
    }
}

/// Whether the first few kilobytes look like something a text panel would only
/// make a mess of. A single NUL settles it; otherwise it is the proportion of
/// control bytes that no text file has many of.
pub fn looks_binary(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let sample = &bytes[..bytes.len().min(4096)];
    let mut suspicious = 0usize;
    for &byte in sample {
        if byte == 0 {
            return true;
        }
        if byte < 7 || (byte > 14 && byte < 32) {
            suspicious += 1;
        }
    }
    suspicious * 100 > sample.len() * 8
}

// ------------------------------------------------------------------- text ---

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextChunk {
    pub content: String,
    /// True only ever on the first chunk; the panel shows a notice instead.
    pub binary: bool,
    pub done: bool,
    pub next_offset: u64,
    pub size: u64,
}

pub async fn text(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
    label: &str,
    offset: u64,
) -> Result<TextChunk, String> {
    let encoding = encoding_of(label);
    let size = transfer::stat(transfers, pane, side, target, path)
        .await?
        .size;

    // Past the end of a file that has an end. A zero-length file falls through
    // and reads nothing, which is the same answer by a shorter route.
    if offset >= size && size > 0 {
        return Ok(TextChunk {
            content: String::new(),
            binary: false,
            done: true,
            next_offset: size,
            size,
        });
    }

    let wanted = size.saturating_sub(offset).min(TEXT_CHUNK as u64).max(1) as usize;
    let bytes = transfer::read_bytes(transfers, pane, side, target, path, offset, wanted).await?;

    // Judged once, on the opening chunk. A run of control bytes in the middle of
    // a large log is not a reason to stop showing it.
    if offset == 0 && looks_binary(&bytes) {
        return Ok(TextChunk {
            content: String::new(),
            binary: true,
            done: true,
            next_offset: bytes.len() as u64,
            size,
        });
    }

    let next_offset = offset + bytes.len() as u64;
    Ok(TextChunk {
        content: decode(&bytes, encoding),
        binary: false,
        done: bytes.is_empty() || next_offset >= size,
        next_offset,
        size,
    })
}

// ------------------------------------------------------------------ image ---

/// How big a picture may be before it is described rather than drawn.
///
/// The bytes cross the IPC boundary as base64 and then live in the webview for
/// as long as the tab does, so the cost of a preview is roughly twice the file.
/// Thirty-two megabytes is ten times the largest screenshot and still leaves
/// the window able to open the next tab.
pub const IMAGE_LIMIT: u64 = 32 * 1024 * 1024;

/*
 * The picture kinds this window can draw, decided by the bytes rather than the
 * name.
 *
 * By the bytes because the name is the user's: a `.png` that is really a PDF
 * would reach the page as a `data:` URL the renderer refuses, and a broken
 * picture reads on screen as a bug in tshell rather than as a fact about the
 * file. It also means a picture with the wrong extension still opens.
 *
 * SVG is deliberately not here. It is text, it is editable, and this pane
 * already has a better view of it than a picture box does.
 *
 * The lengths are checked where a kind needs more than four bytes; everything
 * below compares the first bytes it is given, so a two-byte file cannot panic
 * on a twelve-byte signature.
 */
pub fn image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    // `BM`, and nothing else begins with it among the kinds here.
    if bytes.starts_with(b"BM") && bytes.len() >= 14 {
        return Some("image/bmp");
    }
    // An icon, or a cursor: 00 00 01 00 / 00 00 02 00.
    if bytes.len() >= 4 && bytes[0] == 0 && bytes[1] == 0 && bytes[3] == 0 && (bytes[2] == 1 || bytes[2] == 2) {
        return Some("image/x-icon");
    }
    // The one ISO-BMFF brand worth claiming. `ftyp` is shared with mp4 and
    // friends, so the brand is what decides; HEIC is left out rather than
    // offered to a renderer that may not have it.
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && (&bytes[8..12] == b"avif" || &bytes[8..12] == b"avis")
    {
        return Some("image/avif");
    }
    None
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageFile {
    /// Empty when this is not a picture the window can draw, or one too large.
    pub mime: &'static str,
    /// The base64 payload of a `data:` URL. Empty unless `mime` is set.
    pub data: String,
    pub size: u64,
    /// True when the file *is* a picture and was not sent because of its size.
    /// The page says so rather than falling back to "not a text file", which is
    /// both true and beside the point.
    pub too_big: bool,
}

pub async fn image(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
) -> Result<ImageFile, String> {
    let size = transfer::stat(transfers, pane, side, target, path)
        .await?
        .size;

    // Enough for every signature above and no more: the answer for a JPEG is in
    // its third byte, and this read happens for every binary file the pane
    // opens, picture or not.
    let head = transfer::read_bytes(transfers, pane, side, target, path, 0, 16).await?;
    let mime = match image_mime(&head) {
        Some(mime) => mime,
        None => {
            return Ok(ImageFile {
                mime: "",
                data: String::new(),
                size,
                too_big: false,
            })
        }
    };

    if size > IMAGE_LIMIT {
        return Ok(ImageFile {
            mime: "",
            data: String::new(),
            size,
            too_big: true,
        });
    }

    let bytes = transfer::read_bytes(transfers, pane, side, target, path, 0, size as usize).await?;
    Ok(ImageFile {
        mime,
        data: BASE64.encode(&bytes),
        size,
        too_big: false,
    })
}

// -------------------------------------------------------------------- dbf ---

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DbfField {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub length: usize,
    pub decimal_count: u8,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DbfChunk {
    pub fields: Vec<DbfField>,
    pub rows: Vec<Vec<String>>,
    pub record_count: u32,
    pub next_record: u32,
    pub done: bool,
}

/*
 * The field descriptors, which start at byte 32 and run in 32-byte records
 * until a 0x0D terminator. Each is an 11-byte NUL-padded name, a one-character
 * type, four bytes that mean nothing here, then the length and the decimal
 * count.
 */
fn parse_fields(header: &[u8], encoding: &'static Encoding) -> Vec<DbfField> {
    let mut fields: Vec<DbfField> = Vec::new();
    let mut at = 32usize;
    while at + 32 <= header.len() {
        if header[at] == 0x0d {
            break;
        }
        let raw = &header[at..at + 11];
        let name_bytes = match raw.iter().position(|&byte| byte == 0) {
            Some(end) => &raw[..end],
            None => raw,
        };
        let name = decode(name_bytes, encoding);
        fields.push(DbfField {
            name: if name.is_empty() {
                format!("FIELD_{}", fields.len() + 1)
            } else {
                name
            },
            kind: (header[at + 11] as char).to_string(),
            length: header[at + 16] as usize,
            decimal_count: header[at + 17],
        });
        at += 32;
    }
    fields
}

/// Dates and logicals get read out as what they mean; everything else is the
/// trimmed text as stored. dBase pads every value, so the trim is not optional.
fn format_value(raw: &[u8], field: &DbfField, encoding: &'static Encoding) -> String {
    let value = decode(raw, encoding).trim().to_string();
    if field.kind == "D" && value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit()) {
        return format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..8]);
    }
    if field.kind == "L" {
        match value.to_uppercase().as_str() {
            "Y" | "T" => return "true".to_string(),
            "N" | "F" => return "false".to_string(),
            _ => {}
        }
    }
    value
}

fn parse_rows(
    data: &[u8],
    fields: &[DbfField],
    record_length: usize,
    encoding: &'static Encoding,
) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut at = 0usize;
    while at + record_length <= data.len() {
        let record = &data[at..at + record_length];
        at += record_length;
        // An asterisk in the first byte is dBase's tombstone.
        if record[0] == 0x2a {
            continue;
        }
        let mut cursor = 1usize;
        let mut row = Vec::with_capacity(fields.len());
        for field in fields {
            let end = (cursor + field.length).min(record.len());
            let cell = if cursor < end {
                &record[cursor..end]
            } else {
                &[][..]
            };
            row.push(format_value(cell, field, encoding));
            cursor += field.length;
        }
        rows.push(row);
    }
    rows
}

pub async fn dbf(
    transfers: &Transfers,
    pane: &str,
    side: &str,
    target: &ssh::Target,
    path: &str,
    label: &str,
    record_offset: u32,
) -> Result<DbfChunk, String> {
    let encoding = encoding_of(label);

    // 4KB covers the descriptors of any table anyone previews; the second read
    // only happens for one with an unusually long header.
    let start = transfer::read_bytes(transfers, pane, side, target, path, 0, 4096).await?;
    if start.len() < 32 {
        return Err("invalidDbfFile".to_string());
    }
    let record_count = u32::from_le_bytes([start[4], start[5], start[6], start[7]]);
    let header_length = u16::from_le_bytes([start[8], start[9]]) as usize;
    let record_length = u16::from_le_bytes([start[10], start[11]]) as usize;
    if header_length < 33 || record_length < 1 {
        return Err("invalidDbfFile".to_string());
    }

    let header = if start.len() >= header_length {
        start[..header_length].to_vec()
    } else {
        transfer::read_bytes(transfers, pane, side, target, path, 0, header_length).await?
    };
    let fields = parse_fields(&header, encoding);

    let from = record_offset.min(record_count);
    let count = DBF_ROWS.min(record_count - from);
    if count == 0 {
        return Ok(DbfChunk {
            fields,
            rows: Vec::new(),
            record_count,
            next_record: record_count,
            done: true,
        });
    }

    let at = header_length as u64 + from as u64 * record_length as u64;
    let data = transfer::read_bytes(
        transfers,
        pane,
        side,
        target,
        path,
        at,
        count as usize * record_length,
    )
    .await?;

    let next_record = from + count;
    Ok(DbfChunk {
        rows: parse_rows(&data, &fields, record_length, encoding),
        fields,
        record_count,
        next_record,
        done: next_record >= record_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_is_a_nul_or_enough_control_bytes() {
        assert!(!looks_binary(b""));
        assert!(!looks_binary(b"hello\r\n\tworld"));
        assert!(looks_binary(b"MZ\0\0"), "a NUL settles it on its own");
        // Nine control bytes in a hundred is over the eight percent line.
        let mut noisy = vec![b'a'; 91];
        noisy.extend(std::iter::repeat(0x01).take(9));
        assert!(looks_binary(&noisy));
    }

    #[test]
    fn language_reads_the_extension_not_the_path() {
        assert_eq!(language("/var/log/app.log"), "text");
        assert_eq!(language("C:\\data\\PRICES.DBF"), "dbf");
        assert_eq!(language("/etc/nginx/nginx.conf"), "ini");
        assert_eq!(language("src/main.cpp"), "cpp");
        // A dot in a directory name is not this file's extension.
        assert_eq!(language("/opt/v1.2/README"), "text");
        assert_eq!(language("noextension"), "text");
    }

    /// A minimal dBase III table: one 10-character field, two records, the
    /// second one deleted.
    fn table() -> Vec<u8> {
        let header_length = 32 + 32 + 1;
        let record_length = 1 + 10;
        let mut file = vec![0u8; header_length];
        file[0] = 0x03;
        file[4..8].copy_from_slice(&2u32.to_le_bytes());
        file[8..10].copy_from_slice(&(header_length as u16).to_le_bytes());
        file[10..12].copy_from_slice(&(record_length as u16).to_le_bytes());
        file[32..36].copy_from_slice(b"NAME");
        file[43] = b'C';
        file[48] = 10;
        file[64] = 0x0d;
        file.extend_from_slice(b" alice     ");
        file.extend_from_slice(b"*bob       ");
        file
    }

    #[test]
    fn dbf_fields_and_rows() {
        let file = table();
        let fields = parse_fields(&file[..65], UTF_8);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].name, "NAME");
        assert_eq!(fields[0].kind, "C");
        assert_eq!(fields[0].length, 10);

        let rows = parse_rows(&file[65..], &fields, 11, UTF_8);
        // The deleted record is skipped, and the padding is trimmed.
        assert_eq!(rows, vec![vec!["alice".to_string()]]);
    }

    #[test]
    fn dbf_values_are_read_out_by_type() {
        let date = DbfField {
            name: "D".into(),
            kind: "D".into(),
            length: 8,
            decimal_count: 0,
        };
        assert_eq!(format_value(b"20260812", &date, UTF_8), "2026-08-12");
        // Not eight digits, so it is left exactly as stored.
        assert_eq!(format_value(b"  spam  ", &date, UTF_8), "spam");

        let flag = DbfField {
            kind: "L".into(),
            ..date.clone()
        };
        assert_eq!(format_value(b"T", &flag, UTF_8), "true");
        assert_eq!(format_value(b"f", &flag, UTF_8), "false");
        assert_eq!(format_value(b"?", &flag, UTF_8), "?");
    }

    #[test]
    fn a_picture_is_known_by_its_bytes() {
        assert_eq!(
            image_mime(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0]),
            Some("image/png")
        );
        assert_eq!(image_mime(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(image_mime(b"GIF89a\x01\x00"), Some("image/gif"));
        assert_eq!(image_mime(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Some("image/webp"));
        // A BMP header is 14 bytes, so the signature alone is not enough.
        assert_eq!(image_mime(b"BM\x36\x00\x00\x00\x00\x00\x00\x00\x36\x00\x00\x00"), Some("image/bmp"));
        assert_eq!(image_mime(&[0, 0, 1, 0, 1, 0]), Some("image/x-icon"));
        assert_eq!(
            image_mime(b"\x00\x00\x00\x20ftypavif\x00\x00\x00\x00"),
            Some("image/avif")
        );
    }

    /// The name is the user's; the bytes are the file's. A PDF called `logo.png`
    /// must not become a broken picture on screen.
    #[test]
    fn a_name_that_claims_to_be_a_picture_does_not_make_one() {
        assert_eq!(image_mime(b"%PDF-1.7\n"), None);
        assert_eq!(image_mime(b"<?xml version=\"1.0\"?><svg/>"), None);
        // MP4 shares `ftyp` with the brand check above and must not slip through.
        assert_eq!(image_mime(b"\x00\x00\x00\x20ftypisom\x00\x00\x00\x00"), None);
    }

    /// The read is of a prefix, so a buffer shorter than a signature is a normal
    /// input rather than a panic.
    #[test]
    fn a_short_file_is_not_a_picture() {
        for prefix in [&b""[..], &b"\x89PN"[..], &b"BM"[..], &b"RIFF"[..], &b"\x00\x00\x01"[..]] {
            assert_eq!(image_mime(prefix), None);
        }
    }
}
