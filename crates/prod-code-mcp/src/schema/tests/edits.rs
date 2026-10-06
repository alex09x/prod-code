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
use std::path::{Path, PathBuf};

use super::super::casing::variants;
use super::super::edits::{
    in_comment, make_workspace_edit, make_workspace_edit_with_ends, make_workspace_edit_with_lines,
    overlaps, ranged_edits, replaces_whole_file,
};
use super::super::scan::{edit_for, scan};
use super::super::types::{Occurrence, lsp_end_position};

#[test]
fn the_two_shapes_of_a_rename_answer_are_told_apart() {
    // gopls and the TypeScript server: one edit per occurrence.
    let ranged = serde_json::json!({ "documentChanges": [ {
        "textDocument": { "uri": "file:///w/a.go" },
        "edits": [
            { "range": { "start": { "line": 3, "character": 1 }, "end": { "line": 3, "character": 8 } }, "newText": "TradeID" }
        ]
    } ] });
    let parts = ranged_edits(&ranged);
    assert_eq!(parts.len(), 1);
    assert!(!parts[0].2, "one identifier edit is not a whole file");

    // rust-analyzer: the file's whole new text.
    let whole = serde_json::json!({ "documentChanges": [ {
        "textDocument": { "uri": "file:///w/a.rs" },
        "edits": [
            { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 12, "character": 0 } }, "newText": "fn main() {}\n" }
        ]
    } ] });
    let parts = ranged_edits(&whole);
    assert!(
        parts[0].2,
        "a replacement from the top of the file is the whole file"
    );
}

#[test]
fn an_identifier_at_the_very_start_of_a_file_is_not_a_whole_file_replacement() {
    let edit = serde_json::json!([
        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 8 } }, "newText": "trade_id" }
    ]);
    assert!(!replaces_whole_file(edit.as_array().unwrap()));
}

#[test]
fn edits_that_want_the_same_characters_are_seen_to_overlap() {
    let a = (5, 10, 5, 17);
    assert!(overlaps(a, (5, 12, 5, 20)), "a later start inside it");
    assert!(
        overlaps(a, (5, 0, 5, 11)),
        "an earlier one reaching into it"
    );
    assert!(
        !overlaps(a, (5, 17, 5, 24)),
        "starting where it ends is not an overlap"
    );
    assert!(!overlaps(a, (4, 0, 4, 40)), "another line");
    assert!(
        overlaps(a, (0, 0, u32::MAX, 0)),
        "a whole-file replacement takes everything"
    );
}

#[test]
fn a_comment_marker_depends_on_the_language() {
    let rust = PathBuf::from("/w/src/lib.rs");
    let python = PathBuf::from("/w/app.py");
    let mut texts = BTreeMap::new();
    texts.insert(
        rust.clone(),
        "#[derive(Debug)] // order_id
let order_id = 1;
"
        .to_string(),
    );
    texts.insert(
        python.clone(),
        "# order_id is the key
"
        .to_string(),
    );
    // `#` starts an attribute in Rust, not a comment: the attribute line is not prose.
    let attribute = Occurrence {
        file: rust.clone(),
        line: 2,
        col: 5,
        len: 8,
        variant: 0,
        in_string: false,
    };
    assert!(!in_comment(&texts, &attribute));
    let after_slashes = Occurrence {
        file: rust,
        line: 1,
        col: 21,
        len: 8,
        variant: 0,
        in_string: false,
    };
    assert!(in_comment(&texts, &after_slashes));
    let hash = Occurrence {
        file: python,
        line: 1,
        col: 3,
        len: 8,
        variant: 0,
        in_string: false,
    };
    assert!(in_comment(&texts, &hash), "in Python it is a comment");
}

#[test]
fn an_edit_replaces_exactly_the_occurrence() {
    let v = variants("order_id", "trade_id");
    let text = "  order_id TEXT PRIMARY KEY,\n";
    let found = scan(text, &v, Path::new("schema.sql"));
    let edit = edit_for(&found[0], &v[found[0].variant]);
    assert_eq!(edit["range"]["start"]["character"], 2);
    assert_eq!(edit["range"]["end"]["character"], 10);
    assert_eq!(edit["newText"], "trade_id");
}

#[test]
fn make_workspace_edit_generates_valid_document_changes() {
    let rewritten = vec![
        (
            PathBuf::from("/w1/schema.proto"),
            "message Trade {}\n".to_string(),
        ),
        (
            PathBuf::from("/w2/types.ts"),
            "export interface Trade {}\n".to_string(),
        ),
    ];
    let edit = make_workspace_edit(&rewritten);
    let changes = edit
        .get("documentChanges")
        .and_then(|c| c.as_array())
        .expect("documentChanges array");
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[0]["textDocument"]["uri"], "file:///w1/schema.proto");
    assert_eq!(changes[0]["edits"][0]["newText"], "message Trade {}\n");
    assert_eq!(changes[1]["textDocument"]["uri"], "file:///w2/types.ts");
    assert_eq!(
        changes[1]["edits"][0]["newText"],
        "export interface Trade {}\n"
    );
}

#[test]
fn schema_rename_across_repos_atomic_application() {
    let temp_a = tempfile::tempdir().unwrap();
    let temp_b = tempfile::tempdir().unwrap();
    let root_a = std::fs::canonicalize(temp_a.path()).unwrap();
    let root_b = std::fs::canonicalize(temp_b.path()).unwrap();
    let proto = root_a.join("schema.proto");
    let ts = root_b.join("types.ts");
    std::fs::write(&proto, "message Order { string order_id = 1; }\n").unwrap();
    std::fs::write(&ts, "export interface Order { orderId: string; }\n").unwrap();

    let rewritten = vec![
        (
            proto.clone(),
            "message Order { string trade_id = 1; }\n".to_string(),
        ),
        (
            ts.clone(),
            "export interface Order { tradeId: string; }\n".to_string(),
        ),
    ];
    let multi_edit = make_workspace_edit(&rewritten);
    let roots = [root_a.as_path(), root_b.as_path()];
    let touched =
        crate::refactor::apply_multi_repository_workspace_edit(&roots, &multi_edit).unwrap();
    assert_eq!(touched.len(), 2);
    assert_eq!(
        std::fs::read_to_string(&proto).unwrap(),
        "message Order { string trade_id = 1; }\n"
    );
    assert_eq!(
        std::fs::read_to_string(&ts).unwrap(),
        "export interface Order { tradeId: string; }\n"
    );
}

#[test]
fn workspace_edit_range_spans_pre_apply_document_when_shortened() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let proto = root.join("schema.proto");
    // Original has 10 lines
    let original_content =
        "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10\n";
    std::fs::write(&proto, original_content).unwrap();

    // Rewritten has only 3 lines
    let shortened = "line 1\nshortened 2\nline 3\n";
    let rewritten = vec![(proto.clone(), shortened.to_string())];
    let mut lines = BTreeMap::new();
    lines.insert(proto.clone(), 10);

    let edit = make_workspace_edit_with_lines(&rewritten, &lines);
    let changes = edit["documentChanges"].as_array().unwrap();
    assert_eq!(changes[0]["edits"][0]["range"]["end"]["line"], 10);

    // Apply edit via refactor
    crate::refactor::apply_workspace_edit(&root, &edit).unwrap();
    // File on disk now has 3 lines
    assert_eq!(std::fs::read_to_string(&proto).unwrap().lines().count(), 3);

    // Generating edit post-apply with make_workspace_edit still reports 10 lines because text_before_apply recalls original
    let post_apply_edit = make_workspace_edit(&rewritten);
    let post_changes = post_apply_edit["documentChanges"].as_array().unwrap();
    assert_eq!(post_changes[0]["edits"][0]["range"]["end"]["line"], 10);
}

#[test]
fn lsp_end_position_computes_exact_coordinates() {
    assert_eq!(lsp_end_position(""), (0, 0));
    assert_eq!(lsp_end_position("hello"), (0, 5));
    assert_eq!(lsp_end_position("hello\n"), (1, 0));
    assert_eq!(lsp_end_position("a\nb"), (1, 1));
    assert_eq!(lsp_end_position("a\nb\n"), (2, 0));
    assert_eq!(lsp_end_position("a\r\nb\r\n"), (2, 0));
    assert_eq!(lsp_end_position("\u{1F600}"), (0, 2));
}

#[test]
fn workspace_edit_end_position_for_file_without_trailing_newline() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let proto = root.join("schema.proto");
    // Original has 2 lines WITHOUT a trailing newline: "line 1\nline 2"
    let original_content = "line 1\nline 2";
    std::fs::write(&proto, original_content).unwrap();

    let rewritten = vec![(proto.clone(), "line 1\nupdated 2\nline 3\n".to_string())];
    let mut ends = BTreeMap::new();
    ends.insert(proto.clone(), (1, 6)); // line 1, character 6 ("line 2")

    let edit = make_workspace_edit_with_ends(&rewritten, &ends);
    let changes = edit["documentChanges"].as_array().unwrap();
    assert_eq!(changes[0]["edits"][0]["range"]["end"]["line"], 1);
    assert_eq!(changes[0]["edits"][0]["range"]["end"]["character"], 6);

    // Apply edit via refactor
    crate::refactor::apply_workspace_edit(&root, &edit).unwrap();
    assert_eq!(
        std::fs::read_to_string(&proto).unwrap(),
        "line 1\nupdated 2\nline 3\n"
    );

    // Generating edit post-apply with make_workspace_edit still reports (1, 6) because text_before_apply recalls original
    let post_apply_edit = make_workspace_edit(&rewritten);
    let post_changes = post_apply_edit["documentChanges"].as_array().unwrap();
    assert_eq!(post_changes[0]["edits"][0]["range"]["end"]["line"], 1);
    assert_eq!(post_changes[0]["edits"][0]["range"]["end"]["character"], 6);
}
