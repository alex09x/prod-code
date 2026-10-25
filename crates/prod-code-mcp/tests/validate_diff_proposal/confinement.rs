/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::refusal::refused;
use super::*;
use std::os::unix::fs::symlink;

#[tokio::test]
async fn nearest_existing_ancestor_is_confined_even_with_missing_parents() {
    let workspace = Workspace::new(FILES);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside.rs");
    std::fs::write(&target, "untouched\n").unwrap();
    symlink(outside.path(), workspace.path("escape")).unwrap();
    symlink(outside.path().join("missing"), workspace.path("dangling")).unwrap();
    let gateway = CompilerGateway::start().await;
    for path in [
        "escape/outside.rs",
        "escape/missing/new.rs",
        "dangling/missing/new.rs",
        "src/../../outside.rs",
        "../missing/new.rs",
        "/outside.rs",
    ] {
        for args in [
            json!({"edits":[{"path":path,"new_text":"new\n"}],"compile":true}),
            json!({"diff":format!("--- a/{path}\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n"),"compile":true}),
        ] {
            refused(&workspace, &gateway, args).await;
        }
    }
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");
    assert!(!outside.path().join("missing").exists());
    std::fs::remove_file(workspace.path("escape")).unwrap();
    std::fs::remove_file(workspace.path("dangling")).unwrap();
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn leaf_symlink_deletion_does_not_follow_its_target() {
    let workspace = Workspace::new(FILES);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("outside.rs");
    std::fs::write(&target, "untouched\n").unwrap();
    symlink(&target, workspace.path("src/link.rs")).unwrap();
    let gateway = CompilerGateway::start().await;
    let result = execute_tool(gateway.addr,&workspace.root(),"code_validate_edits",
        json!({"diff":"--- a/src/link.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-untouched\n","compile":true})).await.unwrap();
    assert!(!result.is_error, "{}", text(&result));
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].hypotheses[0].files[0].relative_path,
        "src/link.rs"
    );
    assert!(requests[0].hypotheses[0].files[0].content.is_none());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "untouched\n");
    assert!(workspace.path("src/link.rs").is_symlink());
    std::fs::remove_file(workspace.path("src/link.rs")).unwrap();
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn alias_paths_conflict_and_leaf_replacements_are_refused() {
    let workspace = Workspace::new(FILES);
    symlink(workspace.path("src"), workspace.path("alias")).unwrap();
    symlink(workspace.path("src/lib.rs"), workspace.path("src/link.rs")).unwrap();
    let gateway = CompilerGateway::start().await;
    refused(&workspace,&gateway,json!({"edits":[{"path":"src/lib.rs","new_text":"a"},{"path":"alias/lib.rs","new_text":"b"}],"compile":true})).await;
    refused(
        &workspace,
        &gateway,
        json!({"edits":[{"path":"src/link.rs","new_text":"a"}],"compile":true}),
    )
    .await;
    refused(&workspace, &gateway, json!({"diff":"--- a/src/link.rs\n+++ b/src/link.rs\n@@ -1 +1 @@\n-old\n+new\n", "compile":true})).await;
    std::fs::remove_file(workspace.path("alias")).unwrap();
    std::fs::remove_file(workspace.path("src/link.rs")).unwrap();
    assert_unchanged(&workspace);
}
