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
use crate::refactor::execute::apply_workspace_edit;

/// A batch that moves a whole directory, edits inside it, creates a file in a new directory,
/// deletes a file and a directory tree, and then fails: every path and byte is back, the
/// moved directory at its old name and nothing the batch created left behind.
#[test]
fn a_failure_after_a_directory_move_puts_every_path_and_byte_back() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src/foo")).unwrap();
    std::fs::create_dir_all(root.join("src/stale/deep")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "mod foo;\n").unwrap();
    std::fs::write(root.join("src/foo/mod.rs"), "pub mod a;\n").unwrap();
    std::fs::write(root.join("src/foo/a.rs"), "// \u{1F600}\npub fn a() {}\n").unwrap();
    std::fs::write(root.join("src/gone.rs"), "pub fn gone() {}\n").unwrap();
    std::fs::write(root.join("src/stale/deep/x.rs"), "x\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let gone = root.join("src/gone.rs");
        std::fs::set_permissions(&gone, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(root.join("blocker"), "not a directory\n").unwrap();
    let before = tree(&root);

    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri("src/lib.rs"), "version": null }, "edits": whole("mod bar;\n") },
        { "kind": "rename", "oldUri": uri("src/foo"), "newUri": uri("src/bar"), "options": { "overwrite": false } },
        { "textDocument": { "uri": uri("src/bar/a.rs"), "version": null }, "edits": at(1, 7, 8, "b") },
        { "kind": "create", "uri": uri("src/new/fresh.rs") },
        { "textDocument": { "uri": uri("src/new/fresh.rs"), "version": null }, "edits": at(0, 0, 0, "fresh\n") },
        { "kind": "delete", "uri": uri("src/gone.rs") },
        { "kind": "delete", "uri": uri("src/stale"), "options": { "recursive": true } },
        { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }
    ]});

    let err = apply_workspace_edit(&root, &edit).expect_err("the last write cannot happen");
    assert!(format!("{err:#}").contains("put back"), "{err:#}");
    assert_eq!(tree(&root), before, "every path and byte is as it was");
    crate::sync::clear_sync_cache(&root);
}

/// The same batch without the failing step lands whole, the moved directory with its edit.
#[test]
fn a_directory_move_with_an_edit_inside_it_lands_whole() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src/foo")).unwrap();
    std::fs::create_dir_all(root.join("src/stale/deep")).unwrap();
    std::fs::write(root.join("src/foo/a.rs"), "// \u{1F600}\npub fn a() {}\n").unwrap();
    std::fs::write(root.join("src/stale/deep/x.rs"), "x\n").unwrap();
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "kind": "rename", "oldUri": uri("src/foo"), "newUri": uri("src/bar") },
        { "textDocument": { "uri": uri("src/bar/a.rs"), "version": null }, "edits": at(1, 7, 8, "b") },
        { "kind": "delete", "uri": uri("src/stale"), "options": { "recursive": true } }
    ]});
    apply_workspace_edit(&root, &edit).unwrap();
    assert!(!root.join("src/foo").exists() && !root.join("src/stale").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("src/bar/a.rs")).unwrap(),
        "// \u{1F600}\npub fn b() {}\n"
    );
    let names: Vec<String> = std::fs::read_dir(root.join("src"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["bar"], "nothing set aside is left behind");
    crate::sync::clear_sync_cache(&root);
}

/// A rename onto a file that exists, without `overwrite`, would destroy that file: it is
/// refused, and the edit before it is undone.
#[test]
fn a_rename_onto_an_existing_file_is_refused_and_undone() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::write(root.join("lib.rs"), "pub fn lib() {}\n").unwrap();
    std::fs::write(root.join("other.rs"), "pub fn other() {}\n").unwrap();
    let before = tree(&root);
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri("lib.rs"), "version": null }, "edits": whole("changed\n") },
        { "kind": "rename", "oldUri": uri("other.rs"), "newUri": uri("lib.rs"), "options": { "overwrite": false } }
    ]});
    let err = apply_workspace_edit(&root, &edit).expect_err("lib.rs exists");
    assert!(format!("{err:#}").contains("already exists"), "{err:#}");
    assert_eq!(tree(&root), before);
    crate::sync::clear_sync_cache(&root);
}

/// LSP applies `documentChanges` in order: a text edit names the path as it is at that step.
/// After `a -> b` and `c -> a`, an edit of `a` is an edit of the file that was `c`, and one
/// of `b` is an edit of the file that was `a`.
#[test]
fn an_ordered_edit_names_each_path_as_it_is_at_that_step() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::write(root.join("a.rs"), "was a\n").unwrap();
    std::fs::write(root.join("c.rs"), "was c\n").unwrap();
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("b.rs") },
        { "kind": "rename", "oldUri": uri("c.rs"), "newUri": uri("a.rs") },
        { "textDocument": { "uri": uri("a.rs"), "version": null }, "edits": at(0, 5, 5, ", edited") },
        { "textDocument": { "uri": uri("b.rs"), "version": null }, "edits": at(0, 5, 5, ", edited") }
    ]});
    apply_workspace_edit(&root, &edit).unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("a.rs")).unwrap(),
        "was c, edited\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("b.rs")).unwrap(),
        "was a, edited\n"
    );
    assert!(!root.join("c.rs").exists());
    crate::sync::clear_sync_cache(&root);
}

/// An edit naming a path an earlier step moved away or deleted, and nothing recreated, was
/// computed against the checkout before the edit. Taken in order it names no file; sent to
/// wherever the file went, it could land on another one. It is refused and nothing stays.
#[test]
fn an_edit_naming_a_path_an_earlier_step_vacated_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("old.rs"), "use crate::old;\n").unwrap();
    std::fs::write(root.join("src/a.rs"), "pub fn a() {}\n").unwrap();
    let before = tree(&root);
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edits = [
        serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("old.rs"), "newUri": uri("new.rs") },
            { "textDocument": { "uri": uri("old.rs"), "version": null }, "edits": whole("use crate::new;\n") }
        ]}),
        serde_json::json!({ "documentChanges": [
            { "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") },
            { "textDocument": { "uri": uri("src/a.rs"), "version": null }, "edits": whole("pub fn b() {}\n") }
        ]}),
        serde_json::json!({ "documentChanges": [
            { "kind": "delete", "uri": uri("old.rs") },
            { "textDocument": { "uri": uri("old.rs"), "version": null }, "edits": whole("back\n") }
        ]}),
    ];
    for edit in &edits {
        let err = apply_workspace_edit(&root, edit).expect_err("the path was vacated");
        assert!(format!("{err:#}").contains("earlier step"), "{err:#}");
        assert_eq!(tree(&root), before, "{edit}");
    }
    crate::sync::clear_sync_cache(&root);
}

/// A path a move vacated and a later step created again is a new file: its edit lands
/// there, and the moved file keeps what it had.
#[test]
fn an_old_path_created_again_after_a_move_is_a_new_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("a.rs"), "old a\n").unwrap();
    std::fs::write(root.join("src/x.rs"), "old x\n").unwrap();
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "kind": "rename", "oldUri": uri("a.rs"), "newUri": uri("b.rs") },
        { "kind": "create", "uri": uri("a.rs") },
        { "textDocument": { "uri": uri("a.rs"), "version": null }, "edits": at(0, 0, 0, "new a\n") },
        { "kind": "rename", "oldUri": uri("src"), "newUri": uri("dst") },
        { "kind": "create", "uri": uri("src/x.rs") },
        { "textDocument": { "uri": uri("src/x.rs"), "version": null }, "edits": at(0, 0, 0, "new x\n") }
    ]});
    apply_workspace_edit(&root, &edit).unwrap();
    let read = |rel: &str| std::fs::read_to_string(root.join(rel)).unwrap();
    assert_eq!(read("a.rs"), "new a\n");
    assert_eq!(read("b.rs"), "old a\n");
    assert_eq!(read("src/x.rs"), "new x\n");
    assert_eq!(read("dst/x.rs"), "old x\n");
    crate::sync::clear_sync_cache(&root);
}
