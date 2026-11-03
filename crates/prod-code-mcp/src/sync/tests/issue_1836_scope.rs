/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::{
    apply_pulled_files_for, clear_sync_cache_for, commit_workspace_sync, prepare_workspace_sync_for,
};
use prod_code_protocol::FileDelta;
use std::path::Path;

fn git(root: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn issue_1836_partial_sync_does_not_adopt_or_delete_out_of_scope_writeback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let node = "scope-node.test:9400";
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn value() {}\n").unwrap();
    git(&root, &["init", "-q"]);
    git(&root, &["add", "."]);
    git(
        &root,
        &[
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ],
    );
    let initial = prepare_workspace_sync_for(&root, node, None).unwrap();
    commit_workspace_sync(&root, &initial);
    apply_pulled_files_for(
        &root,
        node,
        &[FileDelta {
            relative_path: "artifacts/out.bin".into(),
            content: Some(vec![1, 2, 3]),
            is_executable: false,
        }],
    )
    .unwrap();
    std::fs::write(
        root.join("prod-code.toml"),
        "[sync]\ninclude=[\"artifacts/**\"]\n",
    )
    .unwrap();
    let partial = prepare_workspace_sync_for(&root, node, Some(Path::new("src/lib.rs"))).unwrap();
    commit_workspace_sync(&root, &partial);
    std::fs::write(root.join("prod-code.toml"), "[sync]\ninclude=[]\n").unwrap();
    let full = prepare_workspace_sync_for(&root, node, None).unwrap();
    assert!(
        !full
            .files
            .iter()
            .any(|f| f.relative_path == "artifacts/out.bin" && f.content.is_none()),
        "a source-only partial sync must not claim ownership and later delete an outside artifact: {:?}",
        full.files
    );
    assert_eq!(
        std::fs::read(root.join("artifacts/out.bin")).unwrap(),
        [1, 2, 3]
    );
    clear_sync_cache_for(&root, node);
}
