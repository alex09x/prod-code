/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::candidates::{
    candidate_names, check_pos, check_span, line_bounds, lines_of, strip_noise,
};
use super::super::facts::decl_at;
use super::super::lsp::{Decl, Pos, Span};

fn span(from: (u32, u32), to: (u32, u32)) -> Span {
    Span::new(Pos::new(from.0, from.1), Pos::new(to.0, to.1))
}

fn decl(name: &str, range: Span, selection: Span) -> Decl {
    Decl {
        name: name.into(),
        kind: "function",
        range,
        selection,
    }
}

fn at(line: u32, character: u32) -> Pos {
    Pos::new(line, character)
}

#[test]
fn candidate_names_skips_keywords_comments_and_strings() {
    let body = "pub fn run(cfg: Config) -> Result<Metrics> {\n    // Metrics of the thing\n    let name = \"Config in a string\";\n    record(cfg, name)\n}";
    let names: Vec<String> = candidate_names(body, 10)
        .into_iter()
        .map(|(n, _, _)| n)
        .collect();
    assert!(names.contains(&"Config".to_string()));
    assert!(names.contains(&"Result".to_string()));
    assert!(names.contains(&"record".to_string()));
    assert!(!names.contains(&"pub".to_string()));
    assert!(!names.contains(&"fn".to_string()));
    assert!(!names.contains(&"let".to_string()));
    // "Metrics" appears in the signature, so its first position is the signature, not
    // the comment; the comment itself contributes nothing new.
    let metrics = candidate_names(body, 10)
        .into_iter()
        .find(|(n, _, _)| n == "Metrics")
        .expect("Metrics is a candidate");
    assert_eq!(metrics.1, 10, "first occurrence is on the signature line");
    assert!(!names.contains(&"string".to_string()));
}

#[test]
fn candidate_positions_are_one_based_and_point_at_the_name() {
    let body = "fn f() {\n    let x = Helper::new();\n}";
    let (name, line, col) = candidate_names(body, 1)
        .into_iter()
        .find(|(n, _, _)| n == "Helper")
        .expect("Helper is a candidate");
    assert_eq!((name.as_str(), line), ("Helper", 2));
    assert_eq!(
        &"    let x = Helper::new();"[col as usize - 1..col as usize + 5],
        "Helper"
    );
}

#[test]
fn candidate_columns_count_utf16_units_after_wide_characters() {
    // `é` is one UTF-16 unit and two bytes, `😀` two units and four bytes; an escaped
    // quote and a comment with a wide character must not shift anything either.
    let body = "let s = \"é\\\"😀\"; helper(ünit); // ☃ tail";
    let names = candidate_names(body, 7);
    let helper = names.iter().find(|(n, _, _)| n == "helper").unwrap();
    let expected = body[..body.find("helper").unwrap()].encode_utf16().count() as u32 + 1;
    assert_eq!((helper.1, helper.2), (7, expected));
    assert!(!names.iter().any(|(n, _, _)| n == "tail"));
    let nit = names.iter().find(|(n, _, _)| n == "nit").unwrap();
    let expected = body[..body.find("nit").unwrap()].encode_utf16().count() as u32 + 1;
    assert_eq!(nit.2, expected);
}

#[test]
fn strip_noise_keeps_byte_offsets() {
    for line in [
        "a \"é😀\\\"x\" b",
        "x // ☃",
        "#[derive(Ü)]",
        "\"unterminated \\",
    ] {
        assert_eq!(strip_noise(line).len(), line.len(), "{line}");
    }
}

#[test]
fn candidate_lines_stop_before_overflowing() {
    let names = candidate_names("alpha\nbeta\ngamma", u32::MAX - 1);
    let lines: Vec<u32> = names.iter().map(|(_, l, _)| *l).collect();
    assert_eq!(lines, vec![u32::MAX - 1, u32::MAX]);
}

#[test]
fn decl_at_picks_the_innermost_declaration() {
    let decls = vec![
        decl("impl Thing", span((9, 0), (59, 1)), span((9, 5), (9, 10))),
        decl("run", span((19, 4), (29, 5)), span((19, 7), (19, 10))),
    ];
    assert_eq!(decl_at(&decls, at(24, 0)).unwrap().name, "run");
    assert_eq!(decl_at(&decls, at(14, 0)).unwrap().name, "impl Thing");
    // A position before the method's first character, on its line, is still the method.
    assert_eq!(decl_at(&decls, at(19, 0)).unwrap().name, "run");
    assert!(decl_at(&decls, at(89, 0)).is_none());
}

#[test]
fn decl_at_prefers_the_name_then_the_covering_range_on_a_shared_line() {
    let decls = vec![
        decl("alpha", span((3, 0), (3, 13)), span((3, 3), (3, 8))),
        decl("beta", span((3, 14), (3, 26)), span((3, 17), (3, 21))),
    ];
    assert_eq!(decl_at(&decls, at(3, 18)).unwrap().name, "beta");
    assert_eq!(decl_at(&decls, at(3, 24)).unwrap().name, "beta");
    assert_eq!(decl_at(&decls, at(3, 4)).unwrap().name, "alpha");
}

#[test]
fn lines_of_is_inclusive_and_one_based() {
    let text = "a\nb\r\nc\nd";
    let lines = line_bounds(text);
    assert_eq!(lines_of(text, &lines, 2, 3).as_deref(), Some("b\nc"));
    assert_eq!(lines_of(text, &lines, 1, 1).as_deref(), Some("a"));
    assert_eq!(lines_of(text, &lines, 4, 4).as_deref(), Some("d"));
    // Lines the text does not have are refused, not cut to the ones it has.
    assert_eq!(lines_of(text, &lines, 3, 5), None);
    assert_eq!(lines_of(text, &lines, 0, 1), None);
    assert_eq!(lines_of(text, &lines, 3, 2), None);
}

#[test]
fn line_bounds_leave_the_line_break_out_and_keep_a_final_empty_line() {
    let text = "ab\r\n\ncd\r\n";
    let lines: Vec<&str> = line_bounds(text).into_iter().map(|r| &text[r]).collect();
    assert_eq!(lines, vec!["ab", "", "cd", ""]);
    // A lone `\r` is not a line break, as in the Rust engine's line index.
    let lone = "a\rb";
    assert_eq!(line_bounds(lone), vec![0..3]);
    assert_eq!(line_bounds(""), vec![0..0]);
}

#[test]
fn check_pos_counts_utf16_units_and_refuses_what_the_source_cannot_hold() {
    let text = "é😀x\r\nfn\n";
    let lines = line_bounds(text);
    // `é` is one unit, `😀` two, `x` one: the first line is 4 units long.
    for character in [0, 1, 3, 4] {
        assert_eq!(
            check_pos(text, &lines, at(0, character)),
            Ok(()),
            "{character}"
        );
    }
    let err = |line, character| check_pos(text, &lines, at(line, character)).unwrap_err();
    assert!(err(0, 2).contains("splits a surrogate pair on line 1"));
    // Unit 5 would be between `\r` and `\n`.
    assert!(err(0, 5).contains("past the end of line 1, which is 4 UTF-16 unit(s) long"));
    assert!(err(0, u32::MAX - 1).contains("past the end of line 1"));
    assert_eq!(
        check_pos(text, &lines, at(2, 0)),
        Ok(()),
        "after the final break"
    );
    assert!(err(2, 1).contains("past the end of line 3"));
    assert!(
        err(3, 0).contains("position 4:1 is past the last line of the source, which has 3 line(s)")
    );
    assert_eq!(
        check_span(text, &lines, span((0, 1), (1, 2))),
        Ok(()),
        "a span inside the text"
    );
    assert!(
        check_span(text, &lines, span((0, 1), (1, 3)))
            .unwrap_err()
            .contains("line 2")
    );
}
