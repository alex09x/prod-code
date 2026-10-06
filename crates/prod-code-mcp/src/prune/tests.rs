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
use std::path::PathBuf;

use crate::dead_code::DeadItem;

use super::edits::{merge, minimal_edits, text_edits};
use super::types::Pruned;

fn edit(sl: u64, sc: u64, el: u64, ec: u64) -> serde_json::Value {
    serde_json::json!({
        "range": { "start": { "line": sl, "character": sc }, "end": { "line": el, "character": ec } },
        "newText": ""
    })
}

#[test]
fn a_deletion_that_overlaps_another_is_left_for_the_next_run() {
    let mut merged = BTreeMap::new();
    assert!(merge(
        &mut merged,
        vec![("a".into(), vec![edit(0, 0, 3, 0)])]
    ));
    assert!(merge(
        &mut merged,
        vec![("a".into(), vec![edit(3, 0, 5, 0)])]
    ));
    assert!(!merge(
        &mut merged,
        vec![("a".into(), vec![edit(2, 0, 4, 0)])]
    ));
    assert!(merge(
        &mut merged,
        vec![("b".into(), vec![edit(2, 0, 4, 0)])]
    ));
    assert_eq!(merged["a"].len(), 2);
}

#[test]
fn two_whole_file_answers_reduce_to_edits_that_do_not_overlap() {
    let old = "a\nfn one() {}\nb\nfn two() {}\nc\n";
    let first = minimal_edits(old, "a\nb\nfn two() {}\nc\n");
    let second = minimal_edits(old, "a\nfn one() {}\nb\nc\n");
    let mut merged = BTreeMap::new();
    assert!(merge(&mut merged, vec![("f".into(), first)]));
    assert!(merge(&mut merged, vec![("f".into(), second)]));
    assert_eq!(
        crate::refactor::apply_text_edits(old, &merged["f"]).unwrap(),
        "a\nb\nc\n"
    );
}

#[test]
fn only_text_edits_are_taken_from_an_answer() {
    let changes = serde_json::json!({ "changes": { "file:///x.rs": [edit(0, 0, 1, 0)] } });
    assert_eq!(text_edits(&changes).unwrap().len(), 1);
    let doc = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": "file:///x.rs", "version": null }, "edits": [edit(0, 0, 1, 0)] }
    ] });
    assert_eq!(text_edits(&doc).unwrap()[0].0, "file:///x.rs");
    let moves =
        serde_json::json!({ "documentChanges": [ { "kind": "delete", "uri": "file:///x.rs" } ] });
    assert!(text_edits(&moves).is_none());
}

#[test]
fn test_generate_git_patch_format() {
    let pruned = Pruned {
        root: PathBuf::from("/workspace"),
        removed: vec![DeadItem {
            name: "unused_helper".into(),
            kind: "function".into(),
            file: "src/lib.rs".into(),
            line: 12,
            col: 4,
            exported: false,
        }],
        skipped: Vec::new(),
        rewritten: vec![("/workspace/src/lib.rs".into(), "fn active() {}\n".into())],
        diagnostics: Vec::new(),
        applied: false,
        symbols_checked: 10,
        unverified: Vec::new(),
        git_patch: None,
        git_commit: None,
    };

    let patch = pruned.generate_git_patch().unwrap();
    assert!(patch.contains("From: Alexander Panasenko <alex@prod.codes>"));
    assert!(patch.contains("Subject: [PATCH] refactor(prune): remove 1 unreferenced orphan(s)"));
    assert!(patch.contains("Pruned 1 orphan(s) of 10 symbol(s) checked:"));
    assert!(patch.contains("  - function unused_helper (src/lib.rs:12)"));
    assert!(patch.contains("diff --git a/src/lib.rs b/src/lib.rs"));
    assert!(patch.contains("-- \nprod-code\n"));
}

#[test]
fn test_generate_git_patch_empty_when_no_rewrites() {
    let pruned = Pruned {
        root: PathBuf::from("/workspace"),
        removed: Vec::new(),
        skipped: Vec::new(),
        rewritten: Vec::new(),
        diagnostics: Vec::new(),
        applied: false,
        symbols_checked: 5,
        unverified: Vec::new(),
        git_patch: None,
        git_commit: None,
    };
    assert!(pruned.generate_git_patch().is_none());
}
