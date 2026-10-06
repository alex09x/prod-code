/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Tests for VFS path normalization, UTF-16 position conversions, and safe file IDs.

use ra_ap_ide::TextSize;
use std::path::{Path, PathBuf};

use crate::engine::RustEngine;
use crate::vfs::{
    MAX_SAFE_FILE_ID, is_safe_file_id, line_col_to_offset, normalize_vfs_path, offset_to_line_col,
    use_path_start,
};

#[test]
fn a_use_item_is_found_behind_its_visibility() {
    assert_eq!(use_path_start("use std::fmt;"), Some(4));
    assert_eq!(use_path_start("pub use sync::scan;"), Some(8));
    assert_eq!(use_path_start("pub(crate) use a::b;"), Some(15));
    assert_eq!(use_path_start("pub(in crate::x) use a::b;"), Some(21));
    assert_eq!(use_path_start("user.name = 1;"), None);
    assert_eq!(use_path_start("pub fn used() {}"), None);
    assert_eq!(use_path_start("public use"), None);
}

#[test]
fn test_rust_detection() {
    let temp = tempfile::tempdir().unwrap();
    assert!(!RustEngine::is_rust_workspace(temp.path()));
    std::fs::write(temp.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
    assert!(RustEngine::is_rust_workspace(temp.path()));
}

#[test]
fn test_line_col_offset_math() {
    let text = "fn main() {\n    println!(\"hello\");\n}\n";
    // Line 1 Col 1 -> 'f' (offset 0)
    assert_eq!(line_col_to_offset(text, 1, 1), Some(TextSize::from(0)));
    assert_eq!(offset_to_line_col(text, TextSize::from(0)), (1, 1));

    // Line 2 Col 5 -> 'p' (offset 16)
    let off = line_col_to_offset(text, 2, 5).unwrap();
    assert_eq!(offset_to_line_col(text, off), (2, 5));
}

/// A column past the end of its own line used to scan through the `\n` into whatever text
/// followed, so an oversized column on a short line silently resolved to a position on a
/// later line, or clamped to the end of the file (#456 follow-up).
#[test]
fn an_oversized_column_is_rejected_not_scanned_into_the_next_line() {
    let text = "ab\ncd\n";
    // Line 1 is "ab": columns 1-3 are valid (3 is just past 'b', before the '\n').
    assert_eq!(line_col_to_offset(text, 1, 3), Some(TextSize::from(2)));
    // Column 4 does not exist on line 1; it must not resolve to 'c' on line 2.
    assert_eq!(line_col_to_offset(text, 1, 4), None);
    assert_eq!(line_col_to_offset(text, 1, 100), None);
    // A line beyond the text (no such line at all) is also rejected, not clamped to EOF.
    assert_eq!(line_col_to_offset(text, 100, 1), None);
}

#[test]
fn a_zero_line_or_column_is_on_no_position() {
    let text = "ab\ncd\n";
    assert_eq!(line_col_to_offset(text, 0, 1), None);
    assert_eq!(line_col_to_offset(text, 1, 0), None);
    assert_eq!(line_col_to_offset("", 0, 1), None);
    assert_eq!(line_col_to_offset("", 1, 0), None);
}

/// Empty text and the empty final line after a trailing `\n` both resolve col 1 to the end
/// of the text, and reject any larger column there.
#[test]
fn empty_text_and_final_empty_lines_are_consistent() {
    assert_eq!(line_col_to_offset("", 1, 1), Some(TextSize::from(0)));
    assert_eq!(line_col_to_offset("", 1, 2), None);
    assert_eq!(line_col_to_offset("", 2, 1), None);

    let text = "ab\n";
    // Line 2 is the empty line after the trailing newline.
    assert_eq!(line_col_to_offset(text, 2, 1), Some(TextSize::from(3)));
    assert_eq!(line_col_to_offset(text, 2, 2), None);

    let text = "ab\n\n";
    // Line 2 is the empty line between the two newlines.
    assert_eq!(line_col_to_offset(text, 2, 1), Some(TextSize::from(3)));
    assert_eq!(line_col_to_offset(text, 2, 2), None);
    // Line 3 is the empty final line after the second newline.
    assert_eq!(line_col_to_offset(text, 3, 1), Some(TextSize::from(4)));
}

/// The columns the gateway answers LSP queries with count UTF-16 units, as a language
/// server's do: past 😀, two of them, the column is one more than the character count, and a
/// CRLF pair is one line break (#456).
#[test]
fn columns_count_utf16_units() {
    let text = "fn a() {}\r\nfn f(😀: u8, x: u8) {}\r\n";
    let x = text.find("x:").unwrap();
    assert_eq!(offset_to_line_col(text, TextSize::from(x as u32)), (2, 14));
    assert_eq!(
        line_col_to_offset(text, 2, 14),
        Some(TextSize::from(x as u32))
    );
    // Between the halves of 😀.
    assert_eq!(line_col_to_offset(text, 2, 7), None);
    let after = text.find(": u8, x").unwrap();
    assert_eq!(
        line_col_to_offset(text, 2, 8),
        Some(TextSize::from(after as u32))
    );
}

/// A CRLF sequence has no source position between its two bytes. The end of the first line
/// is before `\r`, and the next line begins after `\n` (#456).
#[test]
fn a_crlf_pair_has_no_between_bytes_position() {
    for text in ["\r", "ab\r", "😀\r"] {
        let eof = TextSize::of(text);
        let (line, col) = offset_to_line_col(text, eof);
        assert_eq!(line_col_to_offset(text, line, col), Some(eof), "{text:?}");
    }
    let text = "ab\r\ncd";
    assert_eq!(line_col_to_offset(text, 1, 3), Some(TextSize::from(2)));
    assert_eq!(line_col_to_offset(text, 1, 4), None);
    assert_eq!(line_col_to_offset(text, 2, 1), Some(TextSize::from(4)));
    assert_eq!(offset_to_line_col(text, TextSize::from(2)), (1, 3));
    assert_eq!(offset_to_line_col(text, TextSize::from(3)), (1, 3));
    assert_eq!(offset_to_line_col(text, TextSize::from(4)), (2, 1));
}

#[test]
fn test_normalize_vfs_path() {
    let ws = Path::new("/workspace/project");
    assert_eq!(
        normalize_vfs_path(Path::new("src/./main.rs"), ws),
        PathBuf::from("/workspace/project/src/main.rs")
    );
    assert_eq!(
        normalize_vfs_path(Path::new("src/../Cargo.toml"), ws),
        PathBuf::from("/workspace/project/Cargo.toml")
    );
    assert_eq!(
        normalize_vfs_path(Path::new("/workspace/project/src/lib.rs"), ws),
        PathBuf::from("/workspace/project/src/lib.rs")
    );
}

#[test]
fn test_safe_file_id_bounds() {
    assert_eq!(MAX_SAFE_FILE_ID, 0x007F_FFFF);
    const { assert!(MAX_SAFE_FILE_ID < (1 << 24)) };
    assert!(is_safe_file_id(ra_ap_ide::FileId::from_raw(
        MAX_SAFE_FILE_ID
    )));
    assert!(!is_safe_file_id(ra_ap_ide::FileId::from_raw(
        MAX_SAFE_FILE_ID + 1
    )));
}
