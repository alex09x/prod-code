//! Regression test for validating proposed Python files that do not exist on disk (#559).
//! Proposing Python text for an absent file must route to the Python engine (not rust-analyzer VFS),
//! return clean/correct diagnostics, leave the proposed file absent on disk, and preserve existing
//! file validation behavior as a control.

use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn code_validate_edit_validates_absent_python_file_without_writing_to_disk() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"workspace_559\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub fn root_code() {}\n"),
        ("scripts/coverage.py", "def existing_coverage():\n    return 42\n"),
    ]);

    let diagnostics_count = Arc::new(AtomicUsize::new(0));
    let count_clone = Arc::clone(&diagnostics_count);

    let remote = ScriptedGateway::start_arc(Arc::new(move |method, _| match method {
        "textDocument/diagnostic" => {
            count_clone.fetch_add(1, Ordering::SeqCst);
            answers::no_diagnostics()
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let root = ws.root();
    let absent_rel = "scripts/__validation_probe_559.py";
    let absent_abs = root.join(absent_rel);
    assert!(!absent_abs.exists(), "target absent file must not exist before test");

    // 1. Validate absent Python file via public MCP tool `code_validate_edit`
    let result = execute_tool(
        remote,
        &root,
        "code_validate_edit",
        serde_json::json!({
            "path": absent_rel,
            "new_text": "def probe():\n    pass\n",
        }),
    )
    .await
    .expect("validation runs successfully");

    assert!(!result.is_error, "validation must not error: {}", text_of(&result));
    let text = text_of(&result);
    assert!(
        text.contains("0 error(s), 0 warning(s)"),
        "expected clean report, got: {text}"
    );
    assert!(
        !absent_abs.exists(),
        "proposed absent file must remain absent on disk after code_validate_edit"
    );

    // 2. Control: existing Python file `scripts/coverage.py`
    let existing_rel = "scripts/coverage.py";
    let existing_abs = root.join(existing_rel);
    let original_disk_content = std::fs::read_to_string(&existing_abs).unwrap();

    let control_result = execute_tool(
        remote,
        &root,
        "code_validate_edit",
        serde_json::json!({
            "path": existing_rel,
            "new_text": "def updated_coverage():\n    return 100\n",
        }),
    )
    .await
    .expect("control validation runs");

    assert!(!control_result.is_error, "control validation must not error: {}", text_of(&control_result));
    let control_text = text_of(&control_result);
    assert!(
        control_text.contains("0 error(s), 0 warning(s)"),
        "expected clean report for control, got: {control_text}"
    );
    assert_eq!(
        std::fs::read_to_string(&existing_abs).unwrap(),
        original_disk_content,
        "existing file content on disk must not be modified by validation"
    );

    // 3. Multi-file validation via `code_validate_edits` including absent Python file
    let edits_result = execute_tool(
        remote,
        &root,
        "code_validate_edits",
        serde_json::json!({
            "edits": [
                {
                    "path": absent_rel,
                    "new_text": "def probe_multi():\n    pass\n",
                }
            ],
            "also_check": [existing_rel],
        }),
    )
    .await
    .expect("code_validate_edits runs successfully");

    assert!(!edits_result.is_error, "code_validate_edits must not error: {}", text_of(&edits_result));
    let edits_text = text_of(&edits_result);
    assert!(
        edits_text.contains("0 error(s), 0 warning(s)"),
        "expected clean report for multi-file edits, got: {edits_text}"
    );
    assert!(
        !absent_abs.exists(),
        "absent file must remain absent on disk after code_validate_edits"
    );
    assert_eq!(
        std::fs::read_to_string(&existing_abs).unwrap(),
        original_disk_content,
        "existing file content on disk must remain unchanged after code_validate_edits"
    );

    assert!(
        diagnostics_count.load(Ordering::SeqCst) > 0,
        "expected validation to issue at least one diagnostic request"
    );
}
