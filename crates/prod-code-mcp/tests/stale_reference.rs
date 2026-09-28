//! False stale-reference warnings must not attach to doc comments or strings (#484).
//!
//! Renaming or removing a symbol in one file produces stale-reference warnings only for actual
//! code tokens in callers, not in comments, doc comments, string literals, or lifetime parameters.

use prod_code_mcp::diagnostics::STALE_REFERENCE;
use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const CARGO: &str = r#"[package]
name = "stale-ref-repro"
version = "0.1.0"
edition = "2021"
"#;

const LIB_BEFORE: &str = r#"pub fn gap() {}
fn covers() {}
"#;

const LIB_PROPOSED: &str = r#"pub fn gap() {}
"#;

const WORKSPACE_TEXT: &str = r#"/// covers `excess` bytes. Only engines without a session for [`RECLAIM_MIN_IDLE`] are
pub fn work() {}
"#;

const FAKE_SERVER_TEXT: &str = r#"/// for the document waits for the one that covers the text last sent (#293).
pub fn fake() {}
"#;

const CALLER_TEXT: &str = r#"pub fn run() {
    workspace::covers();
}
"#;

fn workspace() -> Workspace {
    Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("crates/engine/src/lib.rs", LIB_BEFORE),
        ("crates/gateway/src/workspace.rs", WORKSPACE_TEXT),
        ("crates/engine/tests/fake_server.rs", FAKE_SERVER_TEXT),
        ("crates/gateway/src/caller.rs", CALLER_TEXT),
    ])
}

fn text_of(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

async fn scripted_gateway() -> SocketAddr {
    let lib_has_covers = Arc::new(AtomicBool::new(true));
    let covers_flag = Arc::clone(&lib_has_covers);
    ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/didOpen" | "textDocument/didChange" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            let text = if method == "textDocument/didOpen" {
                params["textDocument"]["text"].as_str()
            } else {
                params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.first())
                    .and_then(|change| change["text"].as_str())
            };
            if uri.contains("lib.rs")
                && let Some(text) = text
            {
                covers_flag.store(text.contains("covers"), Ordering::SeqCst);
            }
            Value::Null
        }
        "textDocument/documentSymbol" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            if uri.contains("lib.rs") {
                if covers_flag.load(Ordering::SeqCst) {
                    json!([
                        { "name": "gap", "kind": 12 },
                        { "name": "covers", "kind": 12 }
                    ])
                } else {
                    json!([
                        { "name": "gap", "kind": 12 }
                    ])
                }
            } else {
                json!([])
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    }))
    .await
    .addr()
}

#[tokio::test]
async fn validate_texts_removes_false_warnings_in_doc_comments_while_keeping_real_call_flagged() {
    let ws = workspace();
    let remote = scripted_gateway().await;
    let root = ws.root();

    let edits = vec![(
        ws.path("crates/engine/src/lib.rs"),
        LIB_PROPOSED.to_string(),
    )];
    let also_check = vec![
        ws.path("crates/gateway/src/workspace.rs"),
        ws.path("crates/engine/tests/fake_server.rs"),
        ws.path("crates/gateway/src/caller.rs"),
    ];

    let reports = prod_code_mcp::diagnostics::validate_texts(remote, &root, &edits, &also_check)
        .await
        .expect("validation runs");

    assert_eq!(reports.len(), 4);

    // crates/engine/src/lib.rs: clean
    assert_eq!(reports[0].warnings, 0, "{}", reports[0].render());
    assert_eq!(reports[0].errors, 0);

    // crates/gateway/src/workspace.rs: doc comment does NOT produce false stale reference warning
    assert_eq!(
        reports[1].warnings,
        0,
        "false warning in doc comment on workspace.rs must not appear:\n{}",
        reports[1].render()
    );
    assert_eq!(reports[1].errors, 0);

    // crates/engine/tests/fake_server.rs: doc comment does NOT produce false stale reference warning
    assert_eq!(
        reports[2].warnings,
        0,
        "false warning in doc comment on fake_server.rs must not appear:\n{}",
        reports[2].render()
    );
    assert_eq!(reports[2].errors, 0);

    // crates/gateway/src/caller.rs: real call IS flagged with STALE_REFERENCE warning
    assert_eq!(
        reports[3].warnings,
        1,
        "real removed call in caller.rs must be flagged:\n{}",
        reports[3].render()
    );
    let item = &reports[3].items[0];
    assert_eq!(item.code.as_deref(), Some(STALE_REFERENCE));
    assert_eq!(item.line, 2);
    assert_eq!(item.col, 16);
    assert!(
        item.message.contains("covers"),
        "message must name removed symbol: {}",
        item.message
    );
    assert!(
        item.note
            .as_deref()
            .unwrap_or("")
            .contains("crates/engine/src/lib.rs"),
        "note must name source file"
    );
}

#[tokio::test]
async fn code_validate_edits_mcp_removes_false_warnings_in_doc_comments() {
    let ws = workspace();
    let remote = scripted_gateway().await;
    let root = ws.root();

    let result = execute_tool(
        remote,
        &root,
        "code_validate_edits",
        json!({
            "edits": [
                {
                    "path": "crates/engine/src/lib.rs",
                    "new_text": LIB_PROPOSED
                }
            ],
            "also_check": [
                "crates/gateway/src/workspace.rs",
                "crates/engine/tests/fake_server.rs",
                "crates/gateway/src/caller.rs"
            ]
        }),
    )
    .await
    .expect("tool execution succeeds");

    let text = text_of(&result);
    assert!(
        text.contains("crates/gateway/src/workspace.rs: 0 error(s), 0 warning(s)"),
        "workspace.rs must be clean: {text}"
    );
    assert!(
        text.contains("crates/engine/tests/fake_server.rs: 0 error(s), 0 warning(s)"),
        "fake_server.rs must be clean: {text}"
    );
    assert!(
        text.contains("crates/gateway/src/caller.rs: 0 error(s), 1 warning(s)"),
        "caller.rs must have the warning: {text}"
    );
    assert!(
        text.contains("[prod-code::stale-reference]"),
        "must cite stale-reference code: {text}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn a_real_rust_analyzer_ignores_doc_comments_and_flags_real_caller() {
    let remote = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");

    let dir = tempfile::Builder::new()
        .prefix("stale-ref-live-")
        .tempdir()
        .expect("checkout dir");
    let root = std::fs::canonicalize(dir.path()).expect("canonical root");

    const LIVE_CARGO: &str = r#"[package]
name = "stale-ref-live"
version = "0.1.0"
edition = "2021"
"#;

    const LIVE_LIB: &str = r#"pub mod workspace;
pub mod caller;

pub fn gap() {}
pub fn covers() {}
"#;

    const LIVE_LIB_PROPOSED: &str = r#"pub mod workspace;
pub mod caller;

pub fn gap() {}
"#;

    const LIVE_WORKSPACE: &str = r#"/// covers excess bytes
pub fn work() {}
"#;

    const LIVE_CALLER: &str = r#"pub fn call() {
    crate::covers();
}
"#;

    for (rel, text) in [
        ("Cargo.toml", LIVE_CARGO),
        ("src/lib.rs", LIVE_LIB),
        ("src/workspace.rs", LIVE_WORKSPACE),
        ("src/caller.rs", LIVE_CALLER),
    ] {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().expect("parent dir")).expect("mkdir");
        std::fs::write(path, text).expect("write");
    }

    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git runs")
    };
    assert!(git(&["init", "-q"]).success());
    assert!(git(&["add", "-A"]).success());
    git(&[
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=test",
        "commit",
        "-qm",
        "fixture",
    ]);

    let lib = root.join("src/lib.rs");
    let workspace_file = root.join("src/workspace.rs");
    let caller_file = root.join("src/caller.rs");

    let mut loaded = false;
    for _ in 0..120 {
        if let Ok(report) =
            prod_code_mcp::diagnostics::validate_text(remote, &root, &lib, LIVE_LIB).await
            && report.ok()
            && report
                .items
                .iter()
                .all(|d| d.code.as_deref() != Some("unlinked-file"))
        {
            loaded = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(loaded, "analyzer loaded the workspace");

    let compiled = std::process::Command::new("cargo")
        .args(["check", "--quiet"])
        .current_dir(&root)
        .output()
        .expect("compiler checks the original fixture");
    assert!(
        compiled.status.success(),
        "the original caller must compile: {}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let baseline = prod_code_mcp::diagnostics::diagnostics(remote, &root, &caller_file)
        .await
        .expect("valid original caller diagnostics");
    assert_eq!(baseline.errors, 0, "{}", baseline.render());

    let edits = vec![(lib.clone(), LIVE_LIB_PROPOSED.to_string())];
    let also_check = vec![workspace_file.clone(), caller_file.clone()];

    let reports = prod_code_mcp::diagnostics::validate_texts(remote, &root, &edits, &also_check)
        .await
        .expect("validate_texts runs");

    assert_eq!(reports.len(), 3);
    assert_eq!(
        reports[0].warnings,
        0,
        "lib.rs proposed: {}",
        reports[0].render()
    );
    assert_eq!(
        reports[1].warnings,
        0,
        "workspace.rs doc comment must not have stale reference warning: {}",
        reports[1].render()
    );
    assert_eq!(
        reports[1].errors,
        0,
        "workspace.rs must have no errors: {}",
        reports[1].render()
    );

    let caller_flagged = reports[2].items.iter().any(|d| {
        d.code.as_deref() == Some(STALE_REFERENCE)
            || d.note.as_deref().is_some_and(|n| n.contains("covers"))
    });
    assert!(
        caller_flagged,
        "caller.rs must flag real removed call: {}",
        reports[2].render()
    );
}

#[tokio::test]
async fn relocated_definition_is_not_a_stale_caller_but_broken_use_still_is() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", "pub fn covers() {}\n"),
        ("src/caller.rs", "pub fn run() { crate::covers(); }\n"),
    ]);
    let original = Arc::new(AtomicBool::new(true));
    let state = original.clone();
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| {
        let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
        match method {
            "textDocument/didOpen" | "textDocument/didChange" if uri.ends_with("/src/lib.rs") => {
                let text = if method == "textDocument/didOpen" {
                    params["textDocument"]["text"].as_str()
                } else {
                    params["contentChanges"][0]["text"].as_str()
                };
                state.store(text.unwrap_or("").contains("fn covers"), Ordering::SeqCst);
                Value::Null
            }
            "textDocument/documentSymbol" if uri.ends_with("/src/lib.rs") && state.load(Ordering::SeqCst) => json!([{"name":"covers","kind":12}]),
            "textDocument/documentSymbol" if uri.ends_with("/src/moved.rs") => json!([{"name":"covers","kind":12}]),
            "textDocument/documentSymbol" => json!([]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            "textDocument/definition" if uri.ends_with("/src/moved.rs") => json!({"uri":uri,"range":{"start":{"line":0,"character":7},"end":{"line":0,"character":13}}}),
            _ => Value::Null,
        }
    })).await.addr();
    let reports = prod_code_mcp::diagnostics::validate_texts(
        remote,
        &ws.root(),
        &[
            (ws.path("src/lib.rs"), "pub mod moved;\n".to_string()),
            (ws.path("src/moved.rs"), "pub fn covers() {}\n".to_string()),
        ],
        &[ws.path("src/caller.rs")],
    )
    .await
    .unwrap();
    assert_eq!(
        reports[1].warnings,
        0,
        "a moved definition is not a broken use: {}",
        reports[1].render()
    );
    assert!(
        reports[2]
            .items
            .iter()
            .any(|d| d.code.as_deref() == Some(STALE_REFERENCE)),
        "moving a function does not preserve its old path: {}",
        reports[2].render()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running gateway"]
async fn native_inline_test_module_relocation_keeps_definitions_and_fields_clean() {
    let remote = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set native gateway")
        .parse::<SocketAddr>()
        .unwrap();
    const BEFORE: &str = "pub fn api() {}\n#[cfg(test)]\nmod tests {\n fn a() -> u8 { 1 }\n #[test] fn b() { assert_eq!(a(), 1); }\n}\n";
    const AFTER: &str =
        "pub fn api() {}\n#[cfg(test)]\n#[path = \"../tests/unit/moved.rs\"]\nmod tests;\n";
    const MOVED: &str = "fn a() -> u8 { 1 }\n#[test] fn b() { struct Pair { a: u8 } let p = Pair { a: a() }; assert_eq!(p.a, 1); }\n";
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", BEFORE)]);
    let root = ws.root();
    let compile = |dir: &std::path::Path| {
        let result = std::process::Command::new("cargo")
            .args(["test", "--quiet"])
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    compile(&root);
    let mut loaded = false;
    for _ in 0..120 {
        if let Ok(report) =
            prod_code_mcp::diagnostics::validate_text(remote, &root, &ws.path("src/lib.rs"), BEFORE)
                .await
            && report.ok()
            && report
                .items
                .iter()
                .all(|d| d.code.as_deref() != Some("unlinked-file"))
        {
            loaded = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    assert!(loaded, "analyzer loaded baseline");
    let reports = prod_code_mcp::diagnostics::validate_texts(
        remote,
        &root,
        &[
            (ws.path("src/lib.rs"), AFTER.to_string()),
            (ws.path("tests/unit/moved.rs"), MOVED.to_string()),
        ],
        &[],
    )
    .await
    .unwrap();
    for report in reports {
        assert_eq!(
            (report.errors, report.warnings),
            (0, 0),
            "{}",
            report.render()
        );
    }
    assert_eq!(
        std::fs::read_to_string(ws.path("src/lib.rs")).unwrap(),
        BEFORE
    );
    assert!(
        !ws.path("tests/unit/moved.rs").exists(),
        "validation must not write proposal"
    );
    let candidate = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", AFTER),
        ("tests/unit/moved.rs", MOVED),
    ]);
    compile(&candidate.root());
}
