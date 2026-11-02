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
use std::fs;
use std::path::Path;

use prod_code_mcp::patch::apply as patch_apply;
use prod_code_mcp::refactor::edits::planned_texts;
use prod_code_mcp::refactor::execute::apply_workspace_edit;
use serde_json::json;
use url::Url;

/// Snapshot directory tree and file bytes to verify zero unintended disk mutations.
fn tree(root: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read fixture directory") {
            let entry = entry.expect("read fixture entry");
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).expect("read fixture metadata");
            let rel = path.strip_prefix(root).expect("fixture remains under root");
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if meta.is_dir() {
                stack.push(path);
                out.insert(rel_str, None);
            } else {
                out.insert(rel_str, Some(fs::read(&path).expect("read fixture bytes")));
            }
        }
    }
    out
}

fn file_uri(root: &Path, rel: &str) -> String {
    Url::from_file_path(root.join(rel)).unwrap().to_string()
}

fn full_edit(text: &str) -> serde_json::Value {
    json!([{
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 100000, "character": 0 }
        },
        "newText": text
    }])
}

#[test]
fn preview_leaves_local_files_strictly_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    prod_code_mcp::sync::clear_sync_cache(&root);

    fs::create_dir_all(root.join("src")).unwrap();
    let lib_orig = "pub fn value() -> i32 { 10 }\n";
    let extra_orig = "pub fn extra() {}\n";
    fs::write(root.join("src/lib.rs"), lib_orig).unwrap();
    fs::write(root.join("src/extra.rs"), extra_orig).unwrap();

    let before = tree(&root);

    // 1. In-memory WorkspaceEdit preview via planned_texts.
    let edit = json!({
        "documentChanges": [
            {
                "textDocument": { "uri": file_uri(&root, "src/lib.rs"), "version": null },
                "edits": full_edit("pub fn value() -> i32 { 20 }\n")
            },
            {
                "kind": "create",
                "uri": file_uri(&root, "src/created.rs")
            },
            {
                "textDocument": { "uri": file_uri(&root, "src/created.rs"), "version": null },
                "edits": full_edit("pub fn created() {}\n")
            },
            {
                "kind": "delete",
                "uri": file_uri(&root, "src/extra.rs")
            }
        ]
    });

    let (planned, has_resources) = planned_texts(&root, &edit).expect("preview must succeed");
    assert!(has_resources, "create and delete are resource operations");
    assert_eq!(
        planned.len(),
        2,
        "lib.rs and created.rs are in planned texts"
    );

    let planned_map: BTreeMap<_, _> = planned
        .into_iter()
        .map(|(p, t)| {
            (
                p.strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                t,
            )
        })
        .collect();
    assert_eq!(
        planned_map.get("src/lib.rs").map(String::as_str),
        Some("pub fn value() -> i32 { 20 }\n")
    );
    assert_eq!(
        planned_map.get("src/created.rs").map(String::as_str),
        Some("pub fn created() {}\n")
    );

    // Verify filesystem is completely undisturbed by planned_texts.
    assert_eq!(
        tree(&root),
        before,
        "planned_texts must leave disk untouched"
    );

    // 2. In-memory patch preview via patch::apply.
    let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn value() -> i32 { 10 }\n+pub fn value() -> i32 { 30 }\n";
    let patched = patch_apply(&root, diff).expect("patch preview must succeed");
    assert_eq!(patched.texts.len(), 1);
    assert_eq!(patched.texts[0].1, "pub fn value() -> i32 { 30 }\n");

    // Verify filesystem remains 100% identical after diff preview.
    assert_eq!(
        tree(&root),
        before,
        "patch::apply must leave disk untouched"
    );
    prod_code_mcp::sync::clear_sync_cache(&root);
}

#[test]
fn multi_file_workspace_edit_successful_apply() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    prod_code_mcp::sync::clear_sync_cache(&root);

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
    fs::write(root.join("src/old.rs"), "pub fn old() {}\n").unwrap();

    let edit = json!({
        "documentChanges": [
            {
                "textDocument": { "uri": file_uri(&root, "src/main.rs"), "version": null },
                "edits": full_edit("fn main() { renamed::old(); }\n")
            },
            {
                "kind": "rename",
                "oldUri": file_uri(&root, "src/old.rs"),
                "newUri": file_uri(&root, "src/renamed.rs")
            },
            {
                "kind": "create",
                "uri": file_uri(&root, "src/gen/helper.rs")
            },
            {
                "textDocument": { "uri": file_uri(&root, "src/gen/helper.rs"), "version": null },
                "edits": full_edit("pub struct Helper;\n")
            }
        ]
    });

    let touched = apply_workspace_edit(&root, &edit).expect("multi-file apply must succeed");
    assert_eq!(
        touched,
        vec![
            "src/main.rs",
            "src/old.rs",
            "src/renamed.rs",
            "src/gen/helper.rs"
        ]
    );

    assert_eq!(
        fs::read_to_string(root.join("src/main.rs")).unwrap(),
        "fn main() { renamed::old(); }\n"
    );
    assert!(!root.join("src/old.rs").exists());
    assert_eq!(
        fs::read_to_string(root.join("src/renamed.rs")).unwrap(),
        "pub fn old() {}\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("src/gen/helper.rs")).unwrap(),
        "pub struct Helper;\n"
    );

    prod_code_mcp::sync::clear_sync_cache(&root);
}

#[test]
fn rejection_of_invalid_precondition_preserves_unrelated_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    prod_code_mcp::sync::clear_sync_cache(&root);

    fs::create_dir_all(root.join("src")).unwrap();
    let unrelated = "pub fn unrelated() { /* preserve */ }\n";
    let target = "pub fn target() { 1 }\n";
    let binary_bytes = b"\xff\xfe\x00\x00 invalid utf8 binary payload \x80\x90";

    fs::write(root.join("src/unrelated.rs"), unrelated).unwrap();
    fs::write(root.join("src/target.rs"), target).unwrap();
    fs::write(root.join("src/blob.bin"), binary_bytes).unwrap();

    let before = tree(&root);

    // Attempting an edit batch containing an invalid precondition (non-UTF8 file target).
    let edit = json!({
        "documentChanges": [
            {
                "textDocument": { "uri": file_uri(&root, "src/target.rs"), "version": null },
                "edits": full_edit("pub fn target() { 2 }\n")
            },
            {
                "textDocument": { "uri": file_uri(&root, "src/blob.bin"), "version": null },
                "edits": full_edit("attempted text edit\n")
            }
        ]
    });

    let err = apply_workspace_edit(&root, &edit).expect_err("non-utf8 target must be rejected");
    assert!(
        format!("{err:#}").contains("not UTF-8 text"),
        "error must cite non-UTF8 rejection: {err:#}"
    );

    // Assert zero mutations to any files in the workspace (including earlier steps in the batch).
    assert_eq!(
        tree(&root),
        before,
        "precondition failure must preserve all files exactly as they were"
    );

    prod_code_mcp::sync::clear_sync_cache(&root);
}

#[test]
fn backend_rollback_on_partial_failure() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    prod_code_mcp::sync::clear_sync_cache(&root);

    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/first.rs"), "pub fn first() {}\n").unwrap();
    fs::write(root.join("src/second.rs"), "pub fn second() {}\n").unwrap();
    // A regular file placed where a directory would need to be created.
    fs::write(root.join("blocker_file"), "I am a file, not a directory\n").unwrap();

    let before = tree(&root);

    // Step 1: mutates src/first.rs (succeeds)
    // Step 2: renames src/second.rs to src/second_moved.rs (succeeds)
    // Step 3: writes to blocker_file/inner.rs (passes preflight because read_existing treats
    //         NotADirectory as Ok(None), but fails in create_parents during apply)
    let edit = json!({
        "documentChanges": [
            {
                "textDocument": { "uri": file_uri(&root, "src/first.rs"), "version": null },
                "edits": full_edit("pub fn first_mutated() {}\n")
            },
            {
                "kind": "rename",
                "oldUri": file_uri(&root, "src/second.rs"),
                "newUri": file_uri(&root, "src/second_moved.rs")
            },
            {
                "textDocument": { "uri": file_uri(&root, "blocker_file/inner.rs"), "version": null },
                "edits": full_edit("x\n")
            }
        ]
    });

    let err = apply_workspace_edit(&root, &edit)
        .expect_err("step 3 must fail to create parent directory");
    let err_msg = format!("{err:#}");
    assert!(
        err_msg.contains("put back as they were"),
        "error must confirm rollback occurred: {err_msg}"
    );

    // Rollback must restore all prior steps:
    assert_eq!(
        fs::read_to_string(root.join("src/first.rs")).unwrap(),
        "pub fn first() {}\n",
        "first.rs must be restored to its original text"
    );
    assert!(
        root.join("src/second.rs").is_file(),
        "second.rs must be restored"
    );
    assert!(
        !root.join("src/second_moved.rs").exists(),
        "moved destination must be cleaned up"
    );
    assert_eq!(
        fs::read_to_string(root.join("blocker_file")).unwrap(),
        "I am a file, not a directory\n",
        "blocker file must remain undisturbed"
    );

    assert_eq!(
        tree(&root),
        before,
        "backend rollback must leave every file and directory exactly as before"
    );

    prod_code_mcp::sync::clear_sync_cache(&root);
}

#[test]
fn external_editor_race_demonstrates_lack_of_external_coordination() {
    let temp = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(temp.path()).unwrap();
    prod_code_mcp::sync::clear_sync_cache(&root);

    fs::create_dir_all(root.join("src")).unwrap();
    let target = root.join("src/file.rs");
    fs::write(&target, "version 1: original content from agent read\n").unwrap();

    // 1. Agent inspects file at version 1 and prepares an edit replacing it.
    let agent_edit = json!({
        "documentChanges": [
            {
                "textDocument": { "uri": file_uri(&root, "src/file.rs"), "version": null },
                "edits": full_edit("version 1: agent modified content\n")
            }
        ]
    });

    // 2. Interleaved race: an external editor (VS Code, vim, git checkout) writes version 2
    //    between the agent's inspection/hash check and the execution of apply_workspace_edit.
    fs::write(&target, "version 2: external user edit saved in IDE\n").unwrap();

    // 3. Agent calls apply_workspace_edit.
    // Notice: backend writer locks do not coordinate arbitrary external processes.
    // apply_workspace_edit performs no precondition/hash check on target, so it silently
    // overwrites the external editor's modifications.
    let touched = apply_workspace_edit(&root, &agent_edit)
        .expect("apply succeeds without precondition check");
    assert_eq!(touched, vec!["src/file.rs"]);

    let final_content = fs::read_to_string(&target).unwrap();
    assert_eq!(
        final_content, "version 1: agent modified content\n",
        "demonstrates that external editor write was clobbered due to lack of hash precondition"
    );

    prod_code_mcp::sync::clear_sync_cache(&root);
}
