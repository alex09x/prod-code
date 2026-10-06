/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::Modifiers;
use crate::signature_go::edits::edits_by_file;
use crate::signature_go::evidence::parameter_evidence;
use crate::signature_go::execute::change_with;
use crate::signature_go::parse::{body_open, ident_at, identifier_uses, parameter_names_at};
use crate::signature_go::text::{closing, line_col_utf16, map_offset, offset_at, position, splice};
use crate::signature_go::types::TextEdit;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[test]
fn parameter_names_and_body_uses_are_found_by_word() {
    let text = "func F(a, /* b */ b int, c func(b int) int,\n\tdd ...string) (b2 int) {\n\
                    \tx := \"b\" + `b` // b\n\ty := s.b + T{b: 1}.b\n\treturn func() int { return b }()\n}\n";
    let open = text.find('(').unwrap();
    let close = closing(text, open).unwrap();
    let names: Vec<&str> = parameter_names_at(text, open, close)
        .into_iter()
        .map(|at| ident_at(text, at).unwrap())
        .collect();
    assert_eq!(names, vec!["a", "b", "c", "dd"]);
    assert_eq!(parameter_names_at("f()", 1, 2), Vec::<usize>::new());
    assert_eq!(parameter_names_at("f(a int,\n)", 1, 9).len(), 1);
    let body = body_open(text, close + 1).unwrap();
    let end = closing(text, body).unwrap();
    // The struct literal's key counts, the field reads after a dot and the texts do not.
    let uses: Vec<usize> = identifier_uses(text, body, end, "b");
    let lines: Vec<u32> = uses.iter().map(|&o| line_col_utf16(text, o).0).collect();
    assert_eq!(lines, vec![3, 4], "{uses:?}");
    assert!(identifier_uses(text, body, end, "a").is_empty());
    assert!(identifier_uses(text, body, end, "b2").is_empty());
    assert_eq!(identifier_uses(text, body, end, "x").len(), 1);
    assert_eq!(ident_at(text, text.find("dd").unwrap() + 1), None);
    assert_eq!(ident_at(text, text.len()), None);
    assert_eq!(
        position(Path::new("/r"), Path::new("/r/a.go"), text, body),
        "a.go:2:25"
    );
}

/// The parameter's references count as proof only when they are well-formed, current and
/// complete: anything else is an error, and no answer at all is not "unused".
#[test]
fn parameter_references_prove_nothing_unless_complete_and_current() {
    let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let root = std::fs::canonicalize(ws.path()).unwrap();
    let file = root.join("a.go");
    let other = root.join("b.go");
    let text = "package a\n\nfunc F(a, b int) int {\n\treturn a + b\n}\n\nvar b = 1\n";
    std::fs::write(&file, text).unwrap();
    std::fs::write(&other, "package a\n").unwrap();
    let open = text.find("F(").unwrap() + 1;
    let close = closing(text, open).unwrap();
    let at = parameter_names_at(text, open, close)[1];
    let body_at = body_open(text, close + 1).unwrap();
    let body = (body_at, closing(text, body_at).unwrap());
    let loc = |path: &Path, line: u64, character: u64| {
        serde_json::json!({ "uri": format!("file://{}", path.display()),
                "range": { "start": { "line": line, "character": character },
                           "end": { "line": line, "character": character + 1 } } })
    };
    let evidence =
        |answer: serde_json::Value| parameter_evidence(&answer, &file, text, "b", at, body);
    // Unused: the declaration alone. Used: the declaration and the read in the body.
    assert_eq!(
        evidence(serde_json::json!([loc(&file, 2, 10)])).unwrap(),
        Vec::<usize>::new()
    );
    let used = evidence(serde_json::json!([loc(&file, 2, 10), loc(&file, 3, 12)])).unwrap();
    assert_eq!(used, vec![text.find("+ b").unwrap() + 2]);
    let refused = |answer: serde_json::Value| {
        evidence(answer.clone())
            .map(|ok| format!("accepted {answer} as {ok:?}"))
            .unwrap_err()
            .to_string()
    };
    for (answer, said) in [
        (serde_json::Value::Null, "no location"),
        (serde_json::json!([]), "no location"),
        (serde_json::json!({ "uri": "x" }), "no location"),
        (serde_json::json!([{ "uri": 7 }]), "malformed"),
        (
            serde_json::json!([{ "uri": "file:///a.go", "range": { "start": { "line": -1, "character": 0 } } }]),
            "malformed",
        ),
        (
            serde_json::json!([loc(&other, 0, 0)]),
            "outside the function",
        ),
        (serde_json::json!([loc(&file, 2, 11)]), "not on `b`"),
        (serde_json::json!([loc(&file, 40, 0)]), "not on `b`"),
        (
            serde_json::json!([loc(&file, 2, 10), loc(&file, 6, 4)]),
            "outside the function's body",
        ),
        (serde_json::json!([loc(&file, 3, 12)]), "own declaration"),
    ] {
        let err = refused(answer.clone());
        assert!(err.contains(said), "{answer}: {err}");
    }
}

#[test]
fn positions_are_utf16_and_edits_map_through() {
    let text = "a := \"é𝄞\"; f(x, y)\n";
    let at = text.find("f(").unwrap();
    let (l, c) = line_col_utf16(text, at);
    assert_eq!((l, c), (0, 12));
    assert_eq!(offset_at(text, l, c), Some(at));
    assert_eq!(offset_at(text, 0, 8), None, "inside a surrogate pair");
    assert_eq!(offset_at(text, 3, 0), None);
    let crlf = "ab\r\ncd\n";
    assert_eq!(offset_at(crlf, 0, 2), Some(2), "line end precedes CR");
    assert_eq!(offset_at(crlf, 0, 3), None, "CR is not a position");
    assert_eq!(offset_at(crlf, 1, 0), Some(4));
    assert_eq!(offset_at(crlf, 2, 0), Some(7), "empty final line");
    assert_eq!(line_col_utf16(crlf, 2), (0, 2));
    assert_eq!(line_col_utf16(crlf, 4), (1, 0));
    let emoji = "😀\r\n";
    assert_eq!(offset_at(emoji, 0, 0), Some(0));
    assert_eq!(offset_at(emoji, 0, 1), None, "inside a surrogate pair");
    assert_eq!(
        offset_at(emoji, 0, 2),
        Some(4),
        "emoji occupies two UTF-16 units"
    );
    assert_eq!(offset_at(emoji, 0, 3), None, "CR is not a position");
    let open = at + 1;
    let edits = vec![
        (open + 1, open + 2, "yy".to_string()),
        (0, 1, "bb".to_string()),
    ];
    let mut sorted = edits.clone();
    sorted.sort_by_key(|(s, e, _)| (*s, *e));
    assert_eq!(map_offset(&sorted, open), Some(open + 1));
    assert_eq!(map_offset(&sorted, open + 1), None);
    assert_eq!(splice(text, &sorted), "bb := \"é𝄞\"; f(yy, y)\n");
}

#[test]
fn edits_outside_the_checkout_or_moving_files_are_refused() {
    let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let root = std::fs::canonicalize(ws.path()).unwrap();
    std::fs::write(root.join("a.go"), "package a\n").unwrap();
    let outside = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let other = std::fs::canonicalize(outside.path()).unwrap().join("b.go");
    std::fs::write(&other, "package b\n").unwrap();
    let edit = |uri: String| {
        serde_json::json!({ "documentChanges": [ {
                "textDocument": { "uri": uri, "version": 1 },
                "edits": [ { "range": { "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": 7 } }, "newText": "package" } ]
            } ] })
    };
    let mut originals = BTreeMap::new();
    let err = edits_by_file(
        &root,
        &edit(format!("file://{}", other.display())),
        &mut originals,
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("outside the checkout"), "{err}");
    let inside = edits_by_file(
        &root,
        &edit(format!("file://{}", root.join("a.go").display())),
        &mut originals,
    )
    .unwrap();
    assert_eq!(
        inside[&root.join("a.go")],
        vec![(0, 7, "package".to_string())]
    );
    let moves =
        serde_json::json!({ "documentChanges": [ { "kind": "create", "uri": "file:///x.go" } ] });
    assert!(edits_by_file(&root, &moves, &mut originals).is_err());
    let overlapping = serde_json::json!({ "changes": {
            format!("file://{}", root.join("a.go").display()): [
                { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 5 } }, "newText": "x" },
                { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 7 } }, "newText": "y" }
            ] } });
    assert!(
        edits_by_file(&root, &overlapping, &mut originals)
            .unwrap_err()
            .to_string()
            .contains("overlapping")
    );
    assert!(edits_by_file(&root, &serde_json::json!([]), &mut originals).is_err());
}

/// A malformed answer is an error, never "no edits" for a file and never another position:
/// a missing or non-list `edits`, a non-list `documentChanges` or `changes` entry, and a
/// line or column past `u32::MAX`, which a cast would have wrapped to a small number.
#[test]
fn malformed_or_oversized_edits_are_refused_not_defaulted() {
    let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let root = std::fs::canonicalize(ws.path()).unwrap();
    let file = root.join("a.go");
    std::fs::write(&file, "package a\n\nfunc F(x, y int) {}\n").unwrap();
    let uri = format!("file://{}", file.display());
    let mut originals = BTreeMap::new();
    let refused = |edit: serde_json::Value, originals: &mut BTreeMap<PathBuf, String>| {
        edits_by_file(&root, &edit, originals)
            .map(|ok| format!("accepted as {ok:?}"))
            .unwrap_err()
            .to_string()
    };
    let shapes = [
        serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri } } ] }),
        serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri }, "edits": null } ] }),
        serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri }, "edits": { "range": {} } } ] }),
        serde_json::json!({ "changes": { uri.clone(): "func F(y, x int) {}" } }),
        serde_json::json!({ "changes": { uri.clone(): null } }),
    ];
    for shape in shapes {
        let err = refused(shape.clone(), &mut originals);
        assert!(
            err.contains("list") && err.contains("nothing was written"),
            "{shape}: {err}"
        );
    }
    let not_a_list = refused(
        serde_json::json!({ "documentChanges": { "textDocument": { "uri": uri } } }),
        &mut originals,
    );
    assert!(
        not_a_list.contains("`documentChanges` is not a list"),
        "{not_a_list}"
    );
    // `u32::MAX + 1 + n` truncates to `n`: line 2, column 7 is `F`'s parameter list.
    let wrap = 1u64 << 32;
    let oversized = |line: u64, character: u64| {
        serde_json::json!({ "changes": { uri.clone(): [ {
                "range": { "start": { "line": line, "character": character },
                           "end": { "line": 2, "character": 11 } },
                "newText": "y, x"
            } ] } })
    };
    assert!(edits_by_file(&root, &oversized(2, 7), &mut originals).is_ok());
    for (line, character) in [(wrap + 2, 7), (2, wrap + 7), (u64::MAX, 7)] {
        let err = refused(oversized(line, character), &mut originals);
        assert!(
            err.contains("out of range or malformed"),
            "{line}:{character}: {err}"
        );
    }
    let negative = serde_json::json!({ "changes": { uri.clone(): [ {
            "range": { "start": { "line": -1, "character": 7 }, "end": { "line": 2, "character": 11 } },
            "newText": "y, x"
        } ] } });
    assert!(refused(negative, &mut originals).contains("out of range or malformed"));
    // An explicitly empty list is an answer, and stays one.
    let empty = edits_by_file(
            &root,
            &serde_json::json!({ "documentChanges": [ { "textDocument": { "uri": uri }, "edits": [] } ] }),
            &mut originals,
        )
        .unwrap();
    assert_eq!(empty[&file], Vec::<TextEdit>::new());
}

#[test]
fn native_edits_inside_crlf_or_surrogates_are_refused_without_mutation() {
    let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let root = std::fs::canonicalize(ws.path()).unwrap();
    let file = root.join("strict.go");
    std::fs::write(&file, "ab\r\n😀\r\n").unwrap();
    let uri = format!("file://{}", file.display());
    let before = std::fs::read(&file).unwrap();
    let edit = |line: u32, character: u32| {
        serde_json::json!({ "changes": { uri.clone(): [ {
                "range": {
                    "start": { "line": line, "character": character },
                    "end": { "line": 2, "character": 0 }
                },
                "newText": "x"
            } ] } })
    };
    for (line, character) in [(0, 3), (1, 1), (1, 3), (2, 1), (3, 0)] {
        let mut originals = BTreeMap::new();
        let err = edits_by_file(&root, &edit(line, character), &mut originals)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("out of range or malformed") && err.contains("nothing was written"),
            "{line}:{character}: {err}"
        );
        assert_eq!(
            std::fs::read(&file).unwrap(),
            before,
            "{line}:{character} wrote"
        );
    }
}

#[tokio::test]
async fn public_positions_do_not_saturate_zero_to_one() {
    let ws = tempfile::Builder::new().prefix("gosig").tempdir().unwrap();
    let root = std::fs::canonicalize(ws.path()).unwrap();
    let file = root.join("a.go");
    std::fs::write(&file, "package a\nfunc F(a int) {}\n").unwrap();
    let remote = "127.0.0.1:1".parse().unwrap();
    for (line, col) in [(0, 1), (1, 0), (0, 0)] {
        let err = change_with(
            remote,
            &root,
            &file,
            line,
            col,
            &[],
            &Modifiers::default(),
            false,
            false,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("not in the file"), "{line}:{col}: {err}");
    }
}
