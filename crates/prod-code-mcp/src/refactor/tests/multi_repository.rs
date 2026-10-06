/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::helpers::tree;
use crate::refactor::execute::apply_multi_repository_workspace_edit;
use crate::refactor::history::text_before_apply;

#[test]
fn test_multi_repository_workspace_edit_atomic_commit_and_rollback() {
    let temp_a = tempfile::tempdir().unwrap();
    let root_a = std::fs::canonicalize(temp_a.path()).unwrap();
    let temp_b = tempfile::tempdir().unwrap();
    let root_b = std::fs::canonicalize(temp_b.path()).unwrap();

    crate::sync::clear_sync_cache(&root_a);
    crate::sync::clear_sync_cache(&root_b);

    std::fs::create_dir_all(root_a.join("src")).unwrap();
    std::fs::write(root_a.join("src/lib.rs"), "pub fn a() -> u32 { 1 }\n").unwrap();
    std::fs::write(root_a.join("src/helper.rs"), "pub fn h() {}\n").unwrap();

    std::fs::create_dir_all(root_b.join("src")).unwrap();
    std::fs::write(root_b.join("src/lib.rs"), "pub fn b() -> u32 { 2 }\n").unwrap();

    let uri_a = |rel: &str| format!("file://{}/{}", root_a.display(), rel);
    let uri_b = |rel: &str| format!("file://{}/{}", root_b.display(), rel);

    // 1. Successful atomic multi-repo edit
    let successful_edit = serde_json::json!({ "documentChanges": [
        { "kind": "rename", "oldUri": uri_a("src/helper.rs"), "newUri": uri_a("src/renamed_helper.rs") },
        { "textDocument": { "uri": uri_a("src/lib.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                       "newText": "pub fn a() -> u32 { 10 }\n" } ] },
        { "textDocument": { "uri": uri_b("src/lib.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                       "newText": "pub fn b() -> u32 { 20 }\n" } ] },
        { "kind": "create", "uri": uri_b("src/extra.rs") },
        { "textDocument": { "uri": uri_b("src/extra.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                       "newText": "pub fn extra() {}\n" } ] },
    ]});

    let touched =
        apply_multi_repository_workspace_edit(&[&root_a, &root_b], &successful_edit).unwrap();
    assert_eq!(
        touched,
        vec![
            root_a.join("src/helper.rs"),
            root_a.join("src/renamed_helper.rs"),
            root_a.join("src/lib.rs"),
            root_b.join("src/lib.rs"),
            root_b.join("src/extra.rs"),
        ]
    );

    assert!(!root_a.join("src/helper.rs").exists());
    assert_eq!(
        std::fs::read_to_string(root_a.join("src/renamed_helper.rs")).unwrap(),
        "pub fn h() {}\n"
    );
    assert_eq!(
        std::fs::read_to_string(root_a.join("src/lib.rs")).unwrap(),
        "pub fn a() -> u32 { 10 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(root_b.join("src/lib.rs")).unwrap(),
        "pub fn b() -> u32 { 20 }\n"
    );
    assert_eq!(
        std::fs::read_to_string(root_b.join("src/extra.rs")).unwrap(),
        "pub fn extra() {}\n"
    );

    // Verify text_before_apply across repos
    assert_eq!(
        text_before_apply(&root_a.join("src/lib.rs")),
        "pub fn a() -> u32 { 1 }\n"
    );
    assert_eq!(
        text_before_apply(&root_b.join("src/lib.rs")),
        "pub fn b() -> u32 { 2 }\n"
    );

    // 2. Rollback across all repos when a later step in repo_b fails
    let tree_a_before = tree(&root_a);

    // Put a blocker file in repo_b
    std::fs::write(root_b.join("src/blocker"), "not a directory\n").unwrap();
    let tree_b_before_with_blocker = tree(&root_b);

    let failing_edit = serde_json::json!({ "documentChanges": [
        { "textDocument": { "uri": uri_a("src/lib.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                       "newText": "pub fn a() -> u32 { 999 }\n" } ] },
        { "kind": "rename", "oldUri": uri_a("src/renamed_helper.rs"), "newUri": uri_a("src/moved_helper.rs") },
        { "textDocument": { "uri": uri_b("src/lib.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 1, "character": 0 } },
                       "newText": "pub fn b() -> u32 { 999 }\n" } ] },
        // Fails: blocker is a file, cannot create blocker/sub.rs
        { "textDocument": { "uri": uri_b("src/blocker/sub.rs"), "version": null },
          "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } },
                       "newText": "failed\n" } ] },
    ]});

    let err = apply_multi_repository_workspace_edit(&[&root_a, &root_b], &failing_edit)
        .expect_err("the fourth write must fail and trigger all-or-nothing rollback");
    assert!(format!("{err:#}").contains("put back"), "{err:#}");
    assert!(
        format!("{err:#}").contains("across repository roots"),
        "{err:#}"
    );

    // Exact all-or-nothing restoration verification
    assert_eq!(
        tree(&root_a),
        tree_a_before,
        "repo_a was completely restored"
    );
    assert_eq!(
        tree(&root_b),
        tree_b_before_with_blocker,
        "repo_b was completely restored"
    );
    assert_eq!(
        std::fs::read_to_string(root_a.join("src/lib.rs")).unwrap(),
        "pub fn a() -> u32 { 10 }\n"
    );
    assert!(root_a.join("src/renamed_helper.rs").is_file());
    assert!(!root_a.join("src/moved_helper.rs").exists());

    // 3. Pre-flight rejection of paths outside all roots
    let outside_edit = serde_json::json!({ "changes": {
        "file:///tmp/unrelated_outside_repo/foo.rs": []
    }});
    let err_outside = apply_multi_repository_workspace_edit(&[&root_a, &root_b], &outside_edit)
        .expect_err("outside repo edit must fail");
    assert!(
        format!("{err_outside:#}").contains("outside any of the specified repository roots"),
        "{err_outside:#}"
    );

    // 4. Empty roots rejection
    let err_empty = apply_multi_repository_workspace_edit(&[], &successful_edit)
        .expect_err("empty roots must be rejected");
    assert!(
        format!("{err_empty:#}").contains("no repository roots provided"),
        "{err_empty:#}"
    );

    crate::sync::clear_sync_cache(&root_a);
    crate::sync::clear_sync_cache(&root_b);
}
