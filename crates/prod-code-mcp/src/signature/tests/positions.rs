/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;

use crate::signature::modifiers::{in_async_fn, with_async};
use crate::signature::parse::offset_of;
use crate::signature::references::{locate_declaration, whole_file_edit};
use crate::signature::util::{line_col_at, normalize, position_at};

#[test]
fn a_position_becomes_an_offset_and_back() {
    let text = "fn a() {}\nfn b(x: u32) {}\n";
    let offset = offset_of(text, 2, 4).expect("line 2 exists");
    assert_eq!(&text[offset..offset + 1], "b");
    assert_eq!(line_col_at(text, offset), Some((2, 4)));
}

/// An analyzer's column counts UTF-16 units, and a CRLF break is two bytes of the file
/// (#456).
#[test]
fn a_position_counts_utf16_units_and_crlf_bytes() {
    // 😀 is four bytes and two UTF-16 units.
    assert_eq!(offset_of("😀fn", 1, 3), Some(4));
    assert_eq!(offset_of("a\r\nfn", 2, 1), Some(3));
    assert_eq!(offset_of("a\r\nfn", 2, 2), Some(4));
    assert_eq!(offset_of("a\r\nb\r\nfn", 3, 1), Some(6));
    // é is two bytes and one unit; 𝄞 four bytes and two units.
    assert_eq!(offset_of("é𝄞x", 1, 4), Some(6));
    assert_eq!(offset_of("é𝄞x", 1, 5), Some(7));
    // The end of a CRLF line is before its `\r`.
    assert_eq!(offset_of("ab\r\n", 1, 3), Some(2));
    assert_eq!(line_col_at("😀fn", 4), Some((1, 3)));
    assert_eq!(line_col_at("a\r\nfn", 3), Some((2, 1)));
    assert_eq!(line_col_at("ab\r\n", 2), Some((1, 3)));
    // A lone `\r` is a character, not a line break.
    assert_eq!(offset_of("a\rb", 1, 3), Some(2));
    assert_eq!(line_col_at("a\rb", 2), Some((1, 3)));
}

/// A position on no character is refused, never moved to the end of its line or the text.
#[test]
fn a_position_on_no_character_is_refused() {
    assert_eq!(offset_of("abc", 1, 999), None);
    assert_eq!(offset_of("abc", 1, 5), None);
    assert_eq!(offset_of("abc", 1, 4), Some(3));
    assert_eq!(offset_of("abc", 0, 1), None);
    assert_eq!(offset_of("abc", 1, 0), None);
    assert_eq!(offset_of("abc", 2, 1), None);
    assert_eq!(offset_of("abc", u32::MAX, u32::MAX), None);
    // Between the halves of a surrogate pair, at the start and at the end of a line.
    assert_eq!(offset_of("😀fn", 1, 2), None);
    assert_eq!(offset_of("a😀", 1, 3), None);
    assert_eq!(offset_of("a😀", 1, 4), Some(5));
    // Past the `\r` of a CRLF break.
    assert_eq!(offset_of("ab\r\ncd", 1, 4), None);
    // A byte inside a character, past the end, or between `\r` and `\n`.
    assert_eq!(line_col_at("😀fn", 1), None);
    assert_eq!(line_col_at("é", 1), None);
    assert_eq!(line_col_at("abc", 4), None);
    assert_eq!(line_col_at("ab\r\ncd", 3), None);
    let err = position_at("abc", 9).unwrap_err().to_string();
    assert!(err.contains("byte 9 of a 3-byte file"), "{err}");
    assert_eq!(position_at("abc", 3).unwrap(), (1, 4));
}

/// The empty text has one empty line, and so does the end of a text after its last break.
#[test]
fn an_empty_text_and_a_final_empty_line_have_one_position() {
    assert_eq!(offset_of("", 1, 1), Some(0));
    assert_eq!(offset_of("", 1, 2), None);
    assert_eq!(offset_of("", 2, 1), None);
    assert_eq!(line_col_at("", 0), Some((1, 1)));
    assert_eq!(line_col_at("", 1), None);
    assert_eq!(offset_of("a\n", 2, 1), Some(2));
    assert_eq!(offset_of("a\r\n", 2, 1), Some(3));
    assert_eq!(offset_of("a\n", 2, 2), None);
    assert_eq!(offset_of("a\n", 3, 1), None);
    assert_eq!(line_col_at("a\n", 2), Some((2, 1)));
    assert_eq!(line_col_at("a\r\n", 3), Some((2, 1)));
    assert_eq!(offset_of("a", 1, 2), Some(1));
    assert_eq!(line_col_at("a", 1), Some((1, 2)));
}

/// Every byte offset that has a position comes back from it, and every position that has an
/// offset comes back from that.
#[test]
fn offsets_and_positions_are_inverses() {
    for text in [
        "",
        "\n",
        "\r\n",
        "a\r\n\r\nb",
        "😀fn x(é: u8) {}\r\n  let 𝄞 = \"\r\";\n😀\n",
        "\r\r\n\u{FEFF}x",
    ] {
        let mut positions = 0;
        for offset in 0..=text.len() + 1 {
            if let Some((line, col)) = line_col_at(text, offset) {
                positions += 1;
                assert_eq!(
                    offset_of(text, line, col),
                    Some(offset),
                    "{text:?} {offset}"
                );
            }
        }
        let mut offsets = 0;
        for line in 0..6 {
            for col in 0..30 {
                if let Some(offset) = offset_of(text, line, col) {
                    offsets += 1;
                    assert_eq!(
                        line_col_at(text, offset),
                        Some((line, col)),
                        "{text:?} {line}:{col}"
                    );
                }
            }
        }
        assert_eq!(positions, offsets, "{text:?}");
    }
}

#[test]
fn the_signature_is_reported_on_one_line() {
    assert_eq!(normalize("\n    a: u32,\n    b: u32,\n"), "a: u32, b: u32");
}

#[test]
fn async_goes_before_unsafe_and_callers_are_told_apart() {
    let t = "pub unsafe fn raw() {}\n";
    let out = with_async(t, t.find("fn raw").unwrap(), true);
    assert_eq!(out, "pub async unsafe fn raw() {}\n");
    assert_eq!(with_async(&out, out.find("fn raw").unwrap(), false), t);
    let plain = "fn f() {}\n";
    assert_eq!(with_async(plain, 0, true), "async fn f() {}\n");
    let callers = "async fn a() {\n    load(1)\n}\nfn b() {\n    load(2)\n}\n";
    assert!(in_async_fn(callers, callers.find("load(1)").unwrap()));
    assert!(!in_async_fn(callers, callers.find("load(2)").unwrap()));
    assert!(!in_async_fn("load(3)", 0));
}

#[test]
fn a_whole_file_edit_covers_the_file_it_replaces() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.rs");
    std::fs::write(&path, "fn a() {}\nfn b() {}\n").unwrap();
    let mut files = BTreeMap::new();
    files.insert(path.clone(), "fn a() {}\n".to_string());
    let edit = whole_file_edit(&files);
    let change = &edit["documentChanges"][0];
    assert_eq!(change["edits"][0]["range"]["start"]["line"], 0);
    assert_eq!(change["edits"][0]["range"]["end"]["line"], 2);
    assert_eq!(change["edits"][0]["newText"], "fn a() {}\n");
}

#[test]
fn a_declaration_is_found_by_its_text_once_and_only_once() {
    let text =
        "fn caller() { join(1, 2); }\n\nfn join(a: u8, b: u8) {}\nfn joined(a: u8, b: u8) {}\n";
    let (open, close) = locate_declaration(text, "join", "a: u8, b: u8").expect("found");
    assert_eq!(&text[open..close], "a: u8, b: u8");
    assert!(
        text[..open].ends_with("fn join("),
        "the one declared as `join`, not `joined` or the call"
    );
    // Changed, or there twice: not guessed at.
    assert_eq!(locate_declaration(text, "join", "a: u16, b: u8"), None);
    let twice = "fn join(a: u8) {}\nmod m { fn join(a: u8) {} }\n";
    assert_eq!(locate_declaration(twice, "join", "a: u8"), None);
}
