/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn preview_apply_preview_runs_through_public_dispatch_and_keeps_effects() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("sample.rs"), "pub fn answer() -> u32 { 1 }\n").unwrap();
    let result = super::super::execute_tool("127.0.0.1:1".parse().unwrap(), root, "code_workflow", json!({
        "apply":true,"timeout_ms":2000,"steps":[
            {"id":"proposal","tool":"preview","args":{"edits":[{"path":"sample.rs","old":"{ 1 }","new":"{ 2 }"}]}},
            {"id":"write","tool":"apply","args":{"files":{"$ref":"proposal#/files"}}},
            {"id":"after","tool":"preview","args":{"edits":[{"path":"sample.rs","old":"{ 2 }","new":"{ 3 }"}]}}
        ]
    })).await.unwrap();
    let report = super::test_support::report(&result);
    assert_eq!(report["status"], "complete", "{report}");
    assert!(report["steps"][1].get("value").is_none());
    assert_eq!(report["steps"][1]["effects"]["status"], "applied");
    assert_eq!(
        std::fs::read_to_string(root.join("sample.rs")).unwrap(),
        "pub fn answer() -> u32 { 2 }\n"
    );
}

#[tokio::test]
async fn stale_preview_is_a_structured_zero_write_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("sample.rs"), "old\n").unwrap();
    let remote = "127.0.0.1:1".parse().unwrap();
    let preview = mutations::invoke(
        remote,
        root,
        "preview",
        json!({"edits":[{"path":"sample.rs","old":"old","new":"proposed"}]}),
    )
    .await
    .unwrap();
    std::fs::write(root.join("sample.rs"), "old\n// external change\n").unwrap();
    let error = mutations::invoke(remote, root, "apply", json!({"files":preview["files"]}))
        .await
        .unwrap_err();
    let failure = error
        .downcast_ref::<operation_failure::OperationFailure>()
        .unwrap();
    assert_eq!(failure.effects["status"], "refused");
    assert_eq!(failure.effects["writes"], 0);
    assert_eq!(
        std::fs::read_to_string(root.join("sample.rs")).unwrap(),
        "old\n// external change\n"
    );
}

#[tokio::test]
async fn cancelled_lock_wait_does_not_leave_a_pending_write() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("sample.rs"), "old\n").unwrap();
    let remote = "127.0.0.1:1".parse().unwrap();
    let preview = mutations::invoke(
        remote,
        root,
        "preview",
        json!({"edits":[{"path":"sample.rs","old":"old","new":"new"}]}),
    )
    .await
    .unwrap();
    let guard = super::super::workspace_writer::workspace_writer_gate(root)
        .lock_owned()
        .await;
    let result = execute(
        remote,
        root,
        json!({"apply":true,"timeout_ms":20,"steps":[{
            "id":"write","tool":"apply","args":{"files":preview["files"]}
        }]}),
    )
    .await
    .unwrap();
    drop(guard);
    let report = super::test_support::report(&result);
    assert_eq!(report["steps"][0]["effects"]["status"], "not_confirmed");
    assert_eq!(
        std::fs::read_to_string(root.join("sample.rs")).unwrap(),
        "old\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn replacement_preserves_existing_executable_permission_bits() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("script.py");
    std::fs::write(&path, "#!/usr/bin/env python3\nprint(1)\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    let remote = "127.0.0.1:1".parse().unwrap();
    let preview = mutations::invoke(
        remote,
        temp.path(),
        "preview",
        json!({"edits":[{"path":"script.py","old":"print(1)","new":"print(2)"}]}),
    )
    .await
    .unwrap();
    mutations::invoke(
        remote,
        temp.path(),
        "apply",
        json!({"files":preview["files"]}),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o755
    );
}
