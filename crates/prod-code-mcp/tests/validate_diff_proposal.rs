/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "validate_diff_proposal/configured.rs"]
mod configured;
#[cfg(unix)]
#[path = "validate_diff_proposal/confinement.rs"]
mod confinement;
#[path = "validate_diff_proposal/gateway.rs"]
mod gateway;
#[path = "validate_diff_proposal/outcomes.rs"]
mod outcomes;
#[path = "validate_diff_proposal/refusal.rs"]
mod refusal;
#[path = "validate_diff_proposal/workspace.rs"]
mod workspace;
use gateway::{CompilerGateway, FILES, assert_unchanged};
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::Workspace;
use serde_json::json;

const DELETE_CHILD: &str =
    "--- a/src/child.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-pub const VALUE: u32 = 1;\n";
const DELETE_UNUSED: &str =
    "--- a/src/unused.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-pub const UNUSED: u32 = 2;\n";
const INVALID_EDIT: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n pub mod child;\n-pub fn value() -> u32 { child::VALUE }\n+pub fn value() -> u32 { child::VALUE + 1 }\n";
const VALID_EDIT: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1 @@\n-pub mod child;\n-pub fn value() -> u32 { child::VALUE }\n+pub fn value() -> u32 { 2 }\n";

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn check_diff(diff: &str, fails: bool, deleted: &str, text_edits: usize) {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"diff": diff, "compile": true}),
    )
    .await
    .unwrap();
    assert_unchanged(&workspace);
    let report = text(&result);
    assert_eq!(result.is_error, fails, "{report}");
    assert!(report.contains("compiler:"), "compiler must run: {report}");
    if fails {
        assert!(
            report.contains("E0583"),
            "missing Rust module must be diagnosed: {report}"
        );
        assert!(!report.contains("proof verified clean"), "{report}");
    } else {
        assert!(
            report.contains("configured compiler check passed"),
            "{report}"
        );
        assert!(report.contains("cwd=\".\""), "{report}");
        assert!(
            report.contains("exit=0 error=null timed_out=false"),
            "{report}"
        );
    }
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "one complete compiler proposal");
    let files = &requests[0].hypotheses[0].files;
    assert_eq!(files.len(), text_edits + 1);
    assert!(
        files
            .iter()
            .any(|f| f.relative_path == deleted && f.content.is_none()),
        "deletion must reach shadow request: {files:?}"
    );
    assert_eq!(
        files.iter().filter(|f| f.content.is_some()).count(),
        text_edits
    );
}

#[tokio::test]
async fn mixed_diff_rejects_missing_module() {
    check_diff(
        &format!("{INVALID_EDIT}{DELETE_CHILD}"),
        true,
        "src/child.rs",
        1,
    )
    .await;
}

#[tokio::test]
async fn mixed_diff_accepts_removed_module_reference() {
    check_diff(
        &format!("{VALID_EDIT}{DELETE_CHILD}"),
        false,
        "src/child.rs",
        1,
    )
    .await;
}

#[tokio::test]
async fn deletion_only_rejects_missing_module() {
    check_diff(DELETE_CHILD, true, "src/child.rs", 0).await;
}

#[tokio::test]
async fn deletion_only_accepts_unused_file() {
    check_diff(DELETE_UNUSED, false, "src/unused.rs", 0).await;
}

#[tokio::test]
async fn borrow_check_alias_compiles_deletions() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"diff": DELETE_CHILD, "borrow_check": true}),
    )
    .await
    .unwrap();
    assert!(result.is_error, "{}", text(&result));
    assert!(text(&result).contains("E0583"), "{}", text(&result));
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].hypotheses[0].files.len(), 1);
    assert!(requests[0].hypotheses[0].files[0].content.is_none());
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn analyzer_only_deletion_is_explicitly_not_checked() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    for diff in [
        DELETE_CHILD.to_string(),
        format!("{INVALID_EDIT}{DELETE_CHILD}"),
    ] {
        let result = execute_tool(
            gateway.addr,
            &workspace.root(),
            "code_validate_edits",
            json!({"diff": diff}),
        )
        .await
        .unwrap();
        let report = text(&result);
        assert!(
            report.contains("deletion") && report.contains("analyzer"),
            "{report}"
        );
        assert!(!report.contains("proof verified clean"), "{report}");
    }
    assert!(gateway.requests.lock().unwrap().is_empty());
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn workspace_resource_operations_are_not_compiler_verified() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let uri = |path| {
        url::Url::from_file_path(workspace.path(path))
            .unwrap()
            .to_string()
    };
    for operation in [
        json!({"kind": "create", "uri": uri("src/new.rs")}),
        json!({"kind": "rename", "oldUri": uri("src/child.rs"), "newUri": uri("src/new.rs")}),
        json!({"kind": "delete", "uri": uri("src/child.rs")}),
    ] {
        for with_text in [false, true] {
            let mut changes = vec![operation.clone()];
            if with_text {
                changes.push(json!({
                    "textDocument": {"uri": uri("src/lib.rs"), "version": null},
                    "edits": [{"range": {"start": {"line": 1, "character": 0},
                        "end": {"line": 2, "character": 0}},
                        "newText": "pub fn value() -> u32 { child::VALUE + 1 }\n"}]
                }));
            }
            let result = execute_tool(
                gateway.addr,
                &workspace.root(),
                "code_validate_edits",
                json!({"workspace_edit": {"documentChanges": changes}, "compile": true}),
            )
            .await;
            let report = match result {
                Ok(result) => {
                    assert!(result.is_error, "{}", text(&result));
                    text(&result)
                }
                Err(error) => format!("{error:#}"),
            };
            assert!(report.contains("not compiler-verified"), "{report}");
            assert!(!report.contains("proof verified clean"), "{report}");
        }
    }
    assert!(gateway.requests.lock().unwrap().is_empty());
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn text_only_compile_callers_still_work() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    for (tool, arguments) in [
        (
            "code_validate_edit",
            json!({"path": "src/lib.rs", "new_text": FILES[1].1, "compile": true}),
        ),
        (
            "code_validate_edits",
            json!({"edits": [{"path": "src/lib.rs", "new_text": FILES[1].1}], "borrow_check": true}),
        ),
    ] {
        let result = execute_tool(gateway.addr, &workspace.root(), tool, arguments)
            .await
            .unwrap();
        assert!(!result.is_error, "{}", text(&result));
        assert!(text(&result).contains("configured compiler check passed"));
        assert!(!text(&result).contains("full compiler"));
    }
    assert_eq!(gateway.requests.lock().unwrap().len(), 2);
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn text_only_entry_point_preserves_unchanged_baseline_checks() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let (errors, report) =
        prod_code_mcp::tools::compile_check(gateway.addr, &workspace.root(), &[])
            .await
            .unwrap();
    assert_eq!(errors, 0, "{report}");
    assert!(
        report.contains("configured compiler check passed"),
        "{report}"
    );
    assert!(
        gateway.requests.lock().unwrap()[0].hypotheses[0]
            .files
            .is_empty()
    );
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn deletion_paths_cannot_escape_checkout() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    for path in ["../outside.rs", "/outside.rs"] {
        let diff = format!("--- {path}\n+++ /dev/null\n@@ -1 +0,0 @@\n-old\n");
        let result = execute_tool(
            gateway.addr,
            &workspace.root(),
            "code_validate_edits",
            json!({"diff": diff, "compile": true}),
        )
        .await;
        let message = match result {
            Err(error) => format!("{error:#}"),
            Ok(result) => text(&result),
        };
        assert!(message.contains("outside the workspace"), "{message}");
    }
    assert!(gateway.requests.lock().unwrap().is_empty());
    assert_unchanged(&workspace);
}

#[cfg(unix)]
#[tokio::test]
async fn deletion_through_symlink_parent_cannot_escape_checkout() {
    let workspace = Workspace::new(FILES);
    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("outside.rs");
    std::fs::write(&outside_file, "outside\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path("escape")).unwrap();
    let gateway = CompilerGateway::start().await;
    let diff = "--- a/escape/outside.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-outside\n";
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"diff": diff, "compile": true}),
    )
    .await;
    let message = match result {
        Err(error) => format!("{error:#}"),
        Ok(result) => text(&result),
    };
    assert!(message.contains("outside the workspace"), "{message}");
    assert!(gateway.requests.lock().unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(outside_file).unwrap(), "outside\n");
    assert!(workspace.path("escape").is_symlink());
    std::fs::remove_file(workspace.path("escape")).unwrap();
    assert_unchanged(&workspace);
}

/// `json: true` returns the diff validation as one JSON object, with the analyzer reports and
/// the compiler's verdict, and keeps the error status of a rejected proposal (#1456).
#[tokio::test]
async fn diff_validation_answers_in_json_when_asked() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    for (diff, accepted) in [
        (VALID_EDIT.to_string(), true),
        (format!("{INVALID_EDIT}{DELETE_CHILD}"), false),
    ] {
        let result = execute_tool(
            gateway.addr,
            &workspace.root(),
            "code_validate_edits",
            json!({"diff": diff, "compile": true, "json": true}),
        )
        .await
        .unwrap();
        let report: serde_json::Value =
            serde_json::from_str(&text(&result)).expect("one JSON object");
        assert_eq!(result.is_error, !accepted, "{report:#}");
        assert_eq!(report["accepted"], accepted, "{report:#}");
        assert!(report["reports"].is_array(), "{report:#}");
        assert!(
            report["files"].as_u64().is_some_and(|n| n >= 1),
            "{report:#}"
        );
        let compiler_errors = report["compiler"]["errors"].as_u64().expect("compiler ran");
        assert_eq!(compiler_errors == 0, accepted, "{report:#}");
        assert!(
            report["compiler"]["report"]
                .as_str()
                .is_some_and(|r| r.contains("compiler:"))
        );
    }
    assert_unchanged(&workspace);
}
