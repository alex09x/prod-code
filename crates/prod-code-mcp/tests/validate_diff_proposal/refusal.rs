/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::sync::atomic::Ordering;

pub async fn refused(
    workspace: &Workspace,
    gateway: &CompilerGateway,
    args: serde_json::Value,
) -> String {
    let report =
        match execute_tool(gateway.addr, &workspace.root(), "code_validate_edits", args).await {
            Ok(result) => {
                assert!(result.is_error, "{}", text(&result));
                text(&result)
            }
            Err(error) => format!("{error:#}"),
        };
    assert!(gateway.requests.lock().unwrap().is_empty(), "{report}");
    assert_eq!(
        gateway.analyzer_requests.load(Ordering::SeqCst),
        0,
        "{report}"
    );
    report
}

#[tokio::test]
async fn unsupported_git_sections_refuse_the_whole_input_in_both_orders() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    for unsupported in [
        "diff --git a/src/child.rs b/src/renamed.rs\nsimilarity index 100%\nrename from src/child.rs\nrename to src/renamed.rs\n",
        "diff --git a/src/child.rs b/src/copied.rs\nsimilarity index 100%\ncopy from src/child.rs\ncopy to src/copied.rs\n",
        "diff --git a/asset b/asset\nGIT binary patch\nliteral 3\nabc\n",
        "diff --git a/asset b/asset\nBinary files a/asset and b/asset differ\n",
        "diff --git a/src/child.rs b/src/child.rs\nold mode 100644\nnew mode 100755\n",
        "diff --git a/asset b/asset\nindex 1234567..2345678 160000\n",
        "diff --git a/empty b/empty\nnew file mode 120000\n",
        "diff --git a/src/child.rs b/src/child.rs\nindex 1234567..2345678 100644\n",
        "--- a/src/child.rs\n+++ b/src/renamed.rs\n@@ -1 +1 @@\n-pub const VALUE: u32 = 1;\n+pub const VALUE: u32 = 2;\n",
    ] {
        for diff in [
            format!("{unsupported}{INVALID_EDIT}"),
            format!("{INVALID_EDIT}{unsupported}"),
        ] {
            let report = refused(&workspace, &gateway, json!({"diff":diff,"compile":true})).await;
            assert!(report.contains("unsupported"), "{report}");
        }
    }
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn workspace_resource_sections_are_refused_before_any_text_in_either_order() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let uri = |p| {
        url::Url::from_file_path(workspace.path(p))
            .unwrap()
            .to_string()
    };
    let edit = json!({"textDocument":{"uri":uri("src/lib.rs")},"edits":[]});
    for op in [
        json!({"kind":"create","uri":uri("src/new.rs")}),
        json!({"kind":"delete","uri":uri("src/child.rs")}),
        json!({"kind":"rename","oldUri":uri("src/child.rs"),"newUri":uri("src/new.rs")}),
    ] {
        for changes in [vec![edit.clone(), op.clone()], vec![op, edit.clone()]] {
            for compile in [true, false] {
                let report = refused(
                    &workspace,
                    &gateway,
                    json!({"workspace_edit":{"documentChanges":changes},"compile":compile}),
                )
                .await;
                assert!(report.contains("not compiler-verified"), "{report}");
            }
        }
    }
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn conflicting_operations_and_truncated_hunks_are_not_partially_checked() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    for diff in [
        format!("{INVALID_EDIT}{INVALID_EDIT}"),
        format!(
            "{INVALID_EDIT}--- a/src/lib.rs\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-pub mod child;\n-pub fn value() -> u32 {{ child::VALUE }}\n"
        ),
        format!("{INVALID_EDIT}--- /dev/null\n+++ b/src/lib.rs/nested.rs\n@@ -0,0 +1 @@\n+new\n"),
        format!("{INVALID_EDIT}--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,2 @@\n+new\n"),
    ] {
        refused(&workspace, &gateway, json!({"diff":diff,"compile":true})).await;
    }
    refused(&workspace,&gateway,json!({"edits":[{"path":"src/lib.rs","new_text":"a"},{"path":"src/lib.rs","new_text":"b"}],"compile":true})).await;
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn hunk_content_is_not_mistaken_for_metadata_and_all_bytes_survive() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let replacement = "pub fn value() -> u32 { 2 }\n";
    let added = "// héllo\n// rename from x\n// GIT binary patch\n// --- a/literal";
    let diff = format!(
        "{VALID_EDIT}{DELETE_CHILD}--- /dev/null\n+++ b/src/deep/new.rs\n@@ -0,0 +1,4 @@\n+// héllo\n+// rename from x\n+// GIT binary patch\n+// --- a/literal\n\\ No newline at end of file\n"
    );
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"diff":diff,"compile":true}),
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{}", text(&result));
    let requests = gateway.requests.lock().unwrap();
    let deltas = &requests[0].hypotheses[0].files;
    assert_eq!(deltas.len(), 3);
    assert_eq!(
        deltas
            .iter()
            .find(|d| d.relative_path == "src/lib.rs")
            .unwrap()
            .content
            .as_deref(),
        Some(replacement.as_bytes())
    );
    assert_eq!(
        deltas
            .iter()
            .find(|d| d.relative_path == "src/deep/new.rs")
            .unwrap()
            .content
            .as_deref(),
        Some(added.as_bytes())
    );
    assert!(
        deltas
            .iter()
            .find(|d| d.relative_path == "src/child.rs")
            .unwrap()
            .content
            .is_none()
    );
    assert_unchanged(&workspace);
    assert!(!workspace.path("src/deep").exists());
}

#[tokio::test]
async fn ordinary_git_metadata_and_header_looking_hunk_lines_work() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let diff = format!(
        "diff --git a/src/lib.rs b/src/lib.rs\nindex 1234567..2345678 100644\n{VALID_EDIT}diff --git a/src/child.rs b/src/child.rs\ndeleted file mode 100644\nindex 1234567..0000000\n{DELETE_CHILD}diff --git a/src/new.rs b/src/new.rs\nnew file mode 100644\n--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,4 @@\n+/*\n+-- literal\n+++ literal\n+*/\n"
    );
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({"diff":diff,"compile":true}),
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{}", text(&result));
    let requests = gateway.requests.lock().unwrap();
    assert_eq!(
        requests[0].hypotheses[0]
            .files
            .iter()
            .find(|d| d.relative_path == "src/new.rs")
            .unwrap()
            .content
            .as_deref(),
        Some(&b"/*\n-- literal\n++ literal\n*/\n"[..])
    );
    assert_unchanged(&workspace);
}

#[tokio::test]
async fn text_workspace_edit_preserves_an_empty_addition_as_content() {
    let workspace = Workspace::new(FILES);
    let gateway = CompilerGateway::start().await;
    let uri = |p| {
        url::Url::from_file_path(workspace.path(p))
            .unwrap()
            .to_string()
    };
    let result = execute_tool(
        gateway.addr,
        &workspace.root(),
        "code_validate_edits",
        json!({
            "workspace_edit":{"documentChanges":[
                {"textDocument":{"uri":uri("src/lib.rs")},"edits":[{
                    "range":{"start":{"line":0,"character":0},"end":{"line":2,"character":0}},
                    "newText":"pub fn value() -> u32 { 2 }\n"}]},
                {"textDocument":{"uri":uri("src/new.rs")},"edits":[]}
            ]},"compile":true
        }),
    )
    .await
    .unwrap();
    assert!(!result.is_error, "{}", text(&result));
    let requests = gateway.requests.lock().unwrap();
    let files = &requests[0].hypotheses[0].files;
    assert_eq!(files.len(), 2);
    assert_eq!(
        files
            .iter()
            .find(|d| d.relative_path == "src/new.rs")
            .unwrap()
            .content
            .as_deref(),
        Some(&b""[..])
    );
    assert_eq!(
        files
            .iter()
            .find(|d| d.relative_path == "src/lib.rs")
            .unwrap()
            .content
            .as_deref(),
        Some(&b"pub fn value() -> u32 { 2 }\n"[..])
    );
    assert_unchanged(&workspace);
}
