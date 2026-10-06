/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::lines::{Lines, path_of, uri_of};
use super::mapping::completion_kind;
use ra_ap_ide::{CompletionItemKind, SymbolKind, TextRange, TextSize};
use serde_json::json;
use std::path::Path;

#[test]
fn positions_count_utf16_units_and_clamp_to_their_line() {
    // "é" is two bytes and one UTF-16 unit; "𝄞" is four bytes and two units.
    let text = "fn a() {}\nlet é = \"𝄞x\";\n";
    let lines = Lines::new(text);
    let x = text.find('x').unwrap();
    assert_eq!(
        lines.position(TextSize::from(x as u32)),
        json!({ "line": 1, "character": 11 })
    );
    assert_eq!(usize::from(lines.offset(1, 11)), x);
    // Past the end of a line is its end, before the newline; past the last line is the end.
    assert_eq!(usize::from(lines.offset(0, 99)), 9);
    assert_eq!(usize::from(lines.offset(9, 0)), text.len());
    assert_eq!(
        lines.range_of(&json!({
            "start": { "line": 1, "character": 11 },
            "end": { "line": 0, "character": 0 }
        })),
        TextRange::new(TextSize::from(0), TextSize::from(x as u32))
    );

    for text in ["\r", "ab\r", "😀\r"] {
        let lines = Lines::new(text);
        let units = text.encode_utf16().count() as u32;
        assert_eq!(
            lines.position(TextSize::of(text)),
            json!({ "line": 0, "character": units })
        );
        assert_eq!(lines.offset(0, units), TextSize::of(text));
    }
    let crlf = "ab\r\ncd";
    let lines = Lines::new(crlf);
    // The two CRLF bytes share the preceding line's end position; no editor response
    // exposes the invalid split between them.
    assert_eq!(
        lines.position(TextSize::from(2)),
        json!({ "line": 0, "character": 2 })
    );
    assert_eq!(
        lines.position(TextSize::from(3)),
        json!({ "line": 0, "character": 2 })
    );
    assert_eq!(usize::from(lines.offset(0, 2)), 2);
    assert_eq!(usize::from(lines.offset(0, 3)), 2);
    assert_eq!(usize::from(lines.offset(1, 0)), 4);
}

#[test]
fn a_completion_kind_is_the_one_rust_analyzer_gives_an_editor() {
    assert_eq!(
        completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Method)),
        2
    );
    assert_eq!(
        completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Function)),
        3
    );
    assert_eq!(
        completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Struct)),
        22
    );
    assert_eq!(
        completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Variant)),
        20
    );
    assert_eq!(
        completion_kind(CompletionItemKind::SymbolKind(SymbolKind::Trait)),
        8
    );
    assert_eq!(completion_kind(CompletionItemKind::Binding), 6);
    assert_eq!(completion_kind(CompletionItemKind::Keyword), 14);
}

#[test]
fn editor_uris_round_trip_spaces_hashes_percents_and_unicode() {
    let path = Path::new("/srv/ws/my app #1/100%41 ü/src/lib.rs");
    let uri = uri_of(path);
    assert_eq!(
        uri,
        "file:///srv/ws/my%20app%20%231/100%2541%20%C3%BC/src/lib.rs"
    );
    assert_eq!(path_of(&uri), path);
    // An editor's own encoding of the same file decodes to it, once.
    assert_eq!(
        path_of("file:///srv/ws/my%20app%20%231/100%2541%20%c3%bc/src/lib.rs"),
        path
    );
}
