/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Comments in the schema files a rename edits as text (#1451): a field mentioned in a SQL or
//! Protobuf comment is prose, reported and left as it is.

use super::types::Occurrence;

/// Whether the occurrence lies in a comment of a SQL (`sql`) or Protobuf (`protobuf`) file,
/// read from the start of the file: `--` (SQL) or `//` (Protobuf) to the end of the line, and
/// `/* … */` in both. Quoted text is skipped, so a marker inside a string starts nothing.
pub(crate) fn in_schema_comment(language: &str, text: &str, occurrence: &Occurrence) -> bool {
    let line_marker: &[u8] = if language == "sql" { b"--" } else { b"//" };
    let Some(end) = byte_offset(text, occurrence) else {
        return false;
    };
    let bytes = text.as_bytes();
    let (mut i, mut quote, mut line, mut block) = (0, None::<u8>, false, false);
    while i < end {
        let byte = bytes[i];
        if line {
            line = byte != b'\n';
        } else if block {
            if bytes[i..].starts_with(b"*/") {
                block = false;
                i += 1;
            }
        } else if let Some(q) = quote {
            if byte == b'\\' && language == "protobuf" {
                i += 1;
            } else if byte == q {
                quote = None;
            }
        } else if bytes[i..].starts_with(line_marker) {
            line = true;
            i += 1;
        } else if bytes[i..].starts_with(b"/*") {
            block = true;
            i += 1;
        } else if matches!(byte, b'\'' | b'"') || (byte == b'`' && language == "sql") {
            quote = Some(byte);
        }
        i += 1;
    }
    line || block
}

/// The byte offset of an occurrence (1-based line, column counted in characters).
pub(crate) fn byte_offset(text: &str, occurrence: &Occurrence) -> Option<usize> {
    let mut start = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index + 1 == occurrence.line as usize {
            let col = occurrence.col.saturating_sub(1) as usize;
            return Some(
                start
                    + line
                        .char_indices()
                        .nth(col)
                        .map_or(line.len(), |(byte, _)| byte),
            );
        }
        start += line.len();
    }
    None
}
