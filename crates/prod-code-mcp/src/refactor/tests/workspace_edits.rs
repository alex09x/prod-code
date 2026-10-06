/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::refactor::edits::apply_text_edits;
use crate::refactor::execute::apply_workspace_edit;
use crate::refactor::history::text_before_apply;

#[test]
fn whole_file_and_ranged_edits() {
    let text = "fn a() {}\nfn b() {}\n";
    let whole = serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 0 } }, "newText": "fn z() {}\n" }]);
    assert_eq!(
        apply_text_edits(text, whole.as_array().unwrap()).unwrap(),
        "fn z() {}\n"
    );
    let ranged = serde_json::json!([
        { "range": { "start": { "line": 0, "character": 3 }, "end": { "line": 0, "character": 4 } }, "newText": "alpha" },
        { "range": { "start": { "line": 1, "character": 3 }, "end": { "line": 1, "character": 4 } }, "newText": "beta" }
    ]);
    assert_eq!(
        apply_text_edits(text, ranged.as_array().unwrap()).unwrap(),
        "fn alpha() {}\nfn beta() {}\n"
    );
}

#[test]
fn apply_workspace_edit_writes_moves_and_records() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "mod old_name;\n").unwrap();
    std::fs::write(root.join("src/old_name.rs"), "pub fn f() {}\n").unwrap();
    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let edit = serde_json::json!({ "documentChanges": [
        { "kind": "rename", "oldUri": uri("src/old_name.rs"), "newUri": uri("src/new_name.rs") },
        { "textDocument": { "uri": uri("src/lib.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } }, "newText": "mod new_name;\n" } ] }
    ]});
    let touched = apply_workspace_edit(&root, &edit).unwrap();
    assert_eq!(
        touched,
        vec!["src/old_name.rs", "src/new_name.rs", "src/lib.rs"]
    );
    assert!(!root.join("src/old_name.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root.join("src/new_name.rs")).unwrap(),
        "pub fn f() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "mod new_name;\n"
    );
    // Nothing the client rewrote counts as synced: every gateway still has the old text.
    let state = crate::sync::load_sync_cache(&root);
    assert!(!state.files.contains_key("src/lib.rs"));
    assert!(!state.files.contains_key("src/new_name.rs"));
    assert!(!state.files.contains_key("src/old_name.rs"));
    // Anything outside the checkout is refused.
    let outside = serde_json::json!({ "changes": { "file:///etc/hosts": [] } });
    assert!(apply_workspace_edit(&root, &outside).is_err());
    crate::sync::clear_sync_cache(&root);
}

#[test]
fn apply_workspace_edit_invalidates_call_hierarchy_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let lib = root.join("src/lib.rs");
    std::fs::write(&lib, "pub fn old() {}\n").unwrap();
    // Seed call hierarchy cache
    {
        let mut lock = crate::call_tree::CALL_CACHE.lock().unwrap();
        lock.insert(
            (
                "127.0.0.1:9000".parse().unwrap(),
                root.to_string_lossy().into_owned(),
                format!("file://{}/src/lib.rs", root.display()),
                1,
                1,
                true,
                1,
            ),
            crate::call_tree::CallCacheEntry {
                edges: serde_json::json!([]),
                timestamp: std::time::Instant::now(),
            },
        );
        assert_eq!(lock.len(), 1);
    }
    let edit = serde_json::json!({ "changes": { format!("file://{}", lib.display()): [
        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
          "newText": "pub fn new() {}\n" }
    ] } });
    let touched = apply_workspace_edit(&root, &edit).unwrap();
    assert_eq!(touched, vec!["src/lib.rs"]);
    // Call hierarchy cache must be cleared after edit is applied
    {
        let lock = crate::call_tree::CALL_CACHE.lock().unwrap();
        assert!(lock.is_empty(), "cache must be cleared on workspace edits");
    }
}

/// A report rendered after an edit was written still has the old text to diff against (#122),
/// and a file changed again since, by anything, is read as it is.
#[test]
fn the_text_before_an_applied_edit_is_kept_until_the_file_changes_again() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    let lib = root.join("src/lib.rs");
    std::fs::write(&lib, "pub fn old() {}\n").unwrap();
    assert_eq!(text_before_apply(&lib), "pub fn old() {}\n");

    let edit = serde_json::json!({ "changes": { format!("file://{}", lib.display()): [
        { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
          "newText": "pub fn new() {}\n" }
    ] } });
    apply_workspace_edit(&root, &edit).unwrap();
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), "pub fn new() {}\n");
    assert_eq!(text_before_apply(&lib), "pub fn old() {}\n");

    std::fs::write(&lib, "pub fn later() {}\n").unwrap();
    assert_eq!(text_before_apply(&lib), "pub fn later() {}\n");
}

/// A multi-file edit either lands whole or not at all. The second write here cannot happen —
/// its parent directory is a regular file — and the first one, which already succeeded, has
/// to be put back, or the checkout is left half-refactored with nothing to say so.
#[test]
fn a_write_that_fails_halfway_leaves_every_file_as_it_was() {
    let temp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(temp.path()).unwrap();
    crate::sync::clear_sync_cache(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn old() {}\n").unwrap();
    std::fs::write(root.join("src/other.rs"), "pub fn keep() {}\n").unwrap();
    // A regular file where a directory would have to be.
    std::fs::write(root.join("blocker"), "not a directory\n").unwrap();

    let uri = |rel: &str| format!("file://{}/{}", root.display(), rel);
    let whole = |text: &str| {
        serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 },
                                         "end": { "line": 1, "character": 0 } },
                              "newText": text }])
    };
    let edit = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri("src/lib.rs"), "version": null }, "edits": whole("pub fn new() {}\n") },
        { "kind": "rename", "oldUri": uri("src/other.rs"), "newUri": uri("src/moved.rs") },
        { "textDocument": { "uri": uri("blocker/inner.rs"), "version": null }, "edits": whole("x\n") }
    ]});

    let err = apply_workspace_edit(&root, &edit).expect_err("the third write cannot happen");
    assert!(
        format!("{err:#}").contains("put back"),
        "the error says the checkout was restored: {err:#}"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("src/lib.rs")).unwrap(),
        "pub fn old() {}\n",
        "the edit that succeeded before the failure is undone"
    );
    assert!(
        root.join("src/other.rs").is_file() && !root.join("src/moved.rs").exists(),
        "the rename is undone too"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("blocker")).unwrap(),
        "not a directory\n",
        "and nothing that was in the way is disturbed"
    );
    crate::sync::clear_sync_cache(&root);
}
