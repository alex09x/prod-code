/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::helpers::{at, tree, whole};
use crate::refactor::edits::planned_texts;
use crate::refactor::execute::apply_workspace_edit;
use crate::refactor::history::text_before_apply;

/// A symlink leading out of the checkout is harmless where it is, but a directory move can
/// carry it under a path a later step writes to. The paths were resolved before the move;
/// each step resolves its own again right before it acts, so nothing outside is touched and
/// the move is undone.
#[cfg(unix)]
#[test]
fn a_symlink_carried_in_by_a_directory_move_is_not_written_through() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let away = std::fs::canonicalize(outside.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("a.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "mod a;\n").unwrap();
    std::os::unix::fs::symlink(&away, root.join("src/link")).unwrap();
    std::fs::write(away.join("keep.rs"), "outside\n").unwrap();
    let (before, away_before) = (tree(&root), tree(&away));
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let moved = serde_json::json!({ "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") });
    let edits = [
        serde_json::json!({ "documentChanges": [ moved,
            { "textDocument": { "uri": uri("dst/link/new.rs"), "version": null }, "edits": whole("escaped\n") } ] }),
        serde_json::json!({ "documentChanges": [ moved,
            { "textDocument": { "uri": uri("dst/link/keep.rs"), "version": null }, "edits": whole("escaped\n") } ] }),
        serde_json::json!({ "documentChanges": [ moved,
            { "kind": "create", "uri": uri("dst/link/new.rs") } ] }),
        serde_json::json!({ "documentChanges": [ moved,
            { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("dst/link/a.rs") } ] }),
        serde_json::json!({ "documentChanges": [ moved,
            { "kind": "delete", "uri": uri("dst/link/keep.rs") } ] }),
    ];
    for edit in &edits {
        let err = apply_workspace_edit(&root, edit).expect_err("the path leads outside");
        assert!(
            format!("{err:#}").contains("outside the checkout"),
            "{err:#}"
        );
        assert_eq!(tree(&root), before, "{edit}");
        assert_eq!(tree(&away), away_before, "{edit}");
    }
    crate::sync::clear_sync_cache(&root);
}

/// A file deleted inside a directory that a later step moves is set aside in that directory
/// and travels with it. Once the edit lands it is gone from the new place, and a failure
/// puts it back at the old one.
#[test]
fn a_file_deleted_before_its_directory_moves_does_not_survive_the_move() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    let seed = || {
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::write(root.join("src/a.rs"), "gone\n").unwrap();
        std::fs::write(root.join("src/deep/d.rs"), "gone too\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "kept\n").unwrap();
    };
    seed();
    std::fs::write(root.join("blocker"), "not a directory\n").unwrap();
    let before = tree(&root);
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let steps = [
        serde_json::json!({ "kind": "delete", "uri": uri("src/a.rs") }),
        serde_json::json!({ "kind": "delete", "uri": uri("src/deep"), "options": { "recursive": true } }),
        serde_json::json!({ "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") }),
    ];
    let mut failing = steps.to_vec();
    failing.push(serde_json::json!(
        { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }));
    let err = apply_workspace_edit(&root, &serde_json::json!({ "documentChanges": failing }))
        .expect_err("the last write cannot happen");
    assert!(format!("{err:#}").contains("put back"), "{err:#}");
    assert_eq!(tree(&root), before, "every path and byte is back");

    apply_workspace_edit(&root, &serde_json::json!({ "documentChanges": steps })).unwrap();
    let mut after: Vec<String> = tree(&root).into_keys().collect();
    after.sort();
    assert_eq!(
        after,
        vec!["blocker", "dst", "dst/b.rs"],
        "nothing set aside survives"
    );
    crate::sync::clear_sync_cache(&root);
}

/// An edit computed before a directory move rewrites the file at its old path, and the move
/// carries it: a report rendered afterwards still has the text it had before (#122).
#[test]
fn the_text_before_an_edit_follows_its_file_through_a_later_directory_move() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src/foo")).unwrap();
    std::fs::write(root.join("src/foo/a.rs"), "pub fn old() {}\n").unwrap();
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri("src/foo/a.rs"), "version": null }, "edits": whole("pub fn new() {}\n") },
        { "kind": "rename", "oldUri": uri("src/foo"), "newUri": uri("src/bar") }
    ]});
    apply_workspace_edit(&root, &edit).unwrap();
    let moved = root.join("src/bar/a.rs");
    assert_eq!(
        std::fs::read_to_string(&moved).unwrap(),
        "pub fn new() {}\n"
    );
    assert_eq!(text_before_apply(&moved), "pub fn old() {}\n");
    crate::sync::clear_sync_cache(&root);
}

/// A file that cannot be read as text is not an empty one: editing it is refused before
/// anything is written, and its bytes stay.
#[test]
fn a_file_that_is_not_text_is_refused_not_overwritten() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::write(root.join("lib.rs"), "pub fn lib() {}\n").unwrap();
    std::fs::write(root.join("blob.rs"), b"\xff\xfe binary\n").unwrap();
    let before = tree(&root);
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri("lib.rs"), "version": null }, "edits": whole("changed\n") },
        { "textDocument": { "uri": uri("blob.rs"), "version": null }, "edits": at(0, 0, 0, "x") }
    ]});
    let err = apply_workspace_edit(&root, &edit).expect_err("blob.rs is not UTF-8");
    assert!(format!("{err:#}").contains("not UTF-8"), "{err:#}");
    assert_eq!(tree(&root), before);
    let unknown = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri("lib.rs"), "version": null }, "edits": whole("changed\n") },
        { "kind": "chmod", "uri": uri("lib.rs") }
    ]});
    let err = apply_workspace_edit(&root, &unknown).expect_err("chmod is not an LSP operation");
    assert!(
        format!("{err:#}").contains("unsupported resource operation"),
        "{err:#}"
    );
    assert_eq!(tree(&root), before);
    crate::sync::clear_sync_cache(&root);
}

/// A path that does not exist yet, under a directory symlinked out of the checkout, is
/// outside it; so is a dangling symlink inside it. Nothing is written through either.
#[cfg(unix)]
#[test]
fn nothing_is_written_through_a_symlink_that_leads_out_of_the_checkout() {
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    let away = std::fs::canonicalize(outside.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::write(root.join("a.rs"), "pub fn a() {}\n").unwrap();
    std::os::unix::fs::symlink(&away, root.join("link")).unwrap();
    std::os::unix::fs::symlink(away.join("missing.rs"), root.join("dangling")).unwrap();
    let before = tree(&root);
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edits = [
        serde_json::json!({ "changes": { uri("link/new.rs"): whole("escaped\n") } }),
        serde_json::json!({ "documentChanges": [ { "kind": "create", "uri": uri("link/sub/new.rs") } ] }),
        serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("link/a.rs") } ] }),
        serde_json::json!({ "changes": { uri("dangling"): whole("escaped\n") } }),
    ];
    for edit in &edits {
        assert!(apply_workspace_edit(&root, edit).is_err(), "{edit}");
        assert!(planned_texts(&root, edit).is_err(), "{edit}");
        assert_eq!(tree(&root), before, "{edit}");
        assert_eq!(tree(&away), Default::default(), "{edit}");
    }
    crate::sync::clear_sync_cache(&root);
}
