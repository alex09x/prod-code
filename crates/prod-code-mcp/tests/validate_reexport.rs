//! A public MCP validation of a real Rust nested-module re-export (#616).
//!
//! This proposal moves an existing item into a newly added child module while retaining its
//! original public path. The whole overlay compiles; a false stale-reference warning means
//! document symbols were compared before the child module was visible to rust-analyzer.

use prod_code_mcp::diagnostics;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

const CARGO: &str =
    "[package]\nname = \"reexport_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const LIB: &str =
    "pub mod shadow;\npub mod tools;\npub mod compile_check;\npub fn probe() -> u32 { 42 }\n";
const SHADOW: &str = "pub async fn run_shadow() -> usize { 42 }\n";
const TOOLS: &str = "pub async fn run() -> usize { crate::shadow::run_shadow().await }\n";
const COMPILE_CHECK: &str = "pub async fn run() -> usize { crate::shadow::run_shadow().await }\n";
const SLICING: &str = "use reexport_fixture::shadow::run_shadow;\n#[test]\nfn sliced_call_still_resolves() { let _ = run_shadow(); }\n";

const SHADOW_PROPOSED: &str = "mod retry;\npub use retry::run_shadow;\npub(crate) async fn run_shadow_once() -> usize { 42 }\n";
const SHADOW_PROPOSED_WITH_NEIGHBOR: &str = "mod retry;\npub use retry::run_shadow; use std::fs;\npub(crate) async fn run_shadow_once() -> usize { 42 }\n";
const RETRY_PROPOSED: &str =
    "use super::run_shadow_once;\npub async fn run_shadow() -> usize { run_shadow_once().await }\n";
const BROKEN_LIB: &str = "pub mod shadow;\npub mod tools;\npub mod compile_check;\npub fn probe() -> u32 { \"not a number\" }\n";

#[tokio::test]
async fn proposed_child_is_open_before_parent_symbols_are_compared() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        ("src/shadow.rs", SHADOW),
        ("src/tools.rs", TOOLS),
        ("src/compile_check.rs", COMPILE_CHECK),
        ("tests/slicing.rs", SLICING),
    ]);
    let state = Arc::new(Mutex::new((false, false, false, false, false)));
    let observed = Arc::clone(&state);
    let retry_uri = url::Url::from_file_path(ws.path("src/shadow/retry.rs"))
        .expect("fixture child URI")
        .to_string();
    let tools_uri = url::Url::from_file_path(ws.path("src/tools.rs"))
        .expect("fixture caller URI")
        .to_string();
    let gateway = ScriptedGateway::start_arc(Arc::new(move |method, params| {
        let mut state = observed.lock().expect("script state");
        match method {
            "textDocument/didOpen" | "textDocument/didChange" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                let text = if method == "textDocument/didOpen" {
                    params["textDocument"]["text"].as_str()
                } else {
                    params["contentChanges"]
                        .as_array()
                        .and_then(|changes| changes.first())
                        .and_then(|change| change["text"].as_str())
                }
                .unwrap_or("");
                if uri.ends_with("/src/shadow.rs") && text.contains("pub use retry::run_shadow") {
                    state.0 = true;
                    state.3 = true;
                    if !state.1 {
                        state.2 = true;
                    }
                }
                if uri.ends_with("/src/shadow/retry.rs") && text.contains("fn run_shadow") {
                    state.1 = true;
                }
                Value::Null
            }
            "textDocument/documentSymbol" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                if uri.ends_with("/src/shadow.rs") {
                    if state.0 && !state.1 {
                        state.2 = true;
                        json!([])
                    } else {
                        json!([{ "name": "run_shadow", "kind": 12 }])
                    }
                } else {
                    json!([])
                }
            }
            "textDocument/diagnostic" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                if state.3 && uri.ends_with("/src/shadow.rs") {
                    json!({
                        "kind": "full",
                        "items": [
                            {
                                "range": {
                                    "start": { "line": 1, "character": 0 },
                                    "end": { "line": 1, "character": 26 }
                                },
                                "severity": 2,
                                "code": "unused_imports",
                                "source": "rust-analyzer",
                                "message": "unused public re-export"
                            },
                            {
                                "range": {
                                    "start": { "line": 1, "character": 27 },
                                    "end": { "line": 1, "character": 39 }
                                },
                                "severity": 2,
                                "code": "unused_imports",
                                "source": "rust-analyzer",
                                "message": "unused std::fs import"
                            }
                        ]
                    })
                } else {
                    answers::no_diagnostics()
                }
            }
            "textDocument/definition" => json!({
                "uri": retry_uri.clone(),
                "range": {
                    "start": { "line": 1, "character": 7 },
                    "end": { "line": 1, "character": 17 }
                }
            }),
            "textDocument/references" => {
                let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
                if uri.ends_with("/src/shadow.rs")
                    && params["position"] == json!({ "line": 1, "character": 15 })
                {
                    state.4 = true;
                    json!([{
                        "uri": tools_uri.clone(),
                        "range": {
                            "start": { "line": 0, "character": 45 },
                            "end": { "line": 0, "character": 55 }
                        }
                    }])
                } else {
                    json!([])
                }
            }
            _ => Value::Null,
        }
    }))
    .await;
    let remote = gateway.addr();
    let root = ws.root();
    let edits = vec![
        (
            ws.path("src/shadow.rs"),
            SHADOW_PROPOSED_WITH_NEIGHBOR.to_string(),
        ),
        (ws.path("src/shadow/retry.rs"), RETRY_PROPOSED.to_string()),
    ];
    let also_check = ["src/tools.rs", "src/compile_check.rs", "tests/slicing.rs"]
        .into_iter()
        .map(|path| ws.path(path))
        .collect::<Vec<_>>();

    let reports = diagnostics::validate_texts(remote, &root, &edits, &also_check)
        .await
        .expect("validation runs");
    assert_eq!(reports.len(), 5);
    assert!(reports.iter().all(|report| report.errors == 0));
    assert_eq!(reports[0].warnings, 1);
    assert_eq!(reports[0].items[0].message, "unused std::fs import");
    assert!(reports.iter().skip(1).all(|report| report.warnings == 0));
    let state = state.lock().expect("script state");
    assert!(
        !state.2,
        "the parent symbols were requested before the new child module was open"
    );
    assert!(
        state.4,
        "reference validation must query the public re-export token itself"
    );
}

#[tokio::test]
async fn unused_public_reexport_warning_is_kept_without_a_checked_reference() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        ("src/shadow.rs", SHADOW),
        ("src/tools.rs", TOOLS),
        ("src/compile_check.rs", COMPILE_CHECK),
    ]);
    let proposed = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&proposed);
    let retry_uri = url::Url::from_file_path(ws.path("src/shadow/retry.rs"))
        .expect("fixture child URI")
        .to_string();
    let gateway = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/didOpen" | "textDocument/didChange" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            let text = if method == "textDocument/didOpen" {
                params["textDocument"]["text"].as_str()
            } else {
                params["contentChanges"]
                    .as_array()
                    .and_then(|changes| changes.first())
                    .and_then(|change| change["text"].as_str())
            }
            .unwrap_or("");
            if uri.ends_with("/src/shadow.rs") && text.contains("pub use retry::run_shadow") {
                *observed.lock().expect("proposal state") = true;
            }
            Value::Null
        }
        "textDocument/documentSymbol" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            if uri.ends_with("/src/shadow.rs") {
                json!([{ "name": "run_shadow", "kind": 12 }])
            } else {
                json!([])
            }
        }
        "textDocument/diagnostic" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            if *observed.lock().expect("proposal state") && uri.ends_with("/src/shadow.rs") {
                json!({
                    "kind": "full",
                    "items": [{
                        "range": {
                            "start": { "line": 1, "character": 0 },
                            "end": { "line": 1, "character": 26 }
                        },
                        "severity": 2,
                        "code": "unused_imports",
                        "source": "rust-analyzer",
                        "message": "unused import"
                    }]
                })
            } else {
                answers::no_diagnostics()
            }
        }
        "textDocument/definition" => json!({
            "uri": retry_uri.clone(),
            "range": {
                "start": { "line": 1, "character": 7 },
                "end": { "line": 1, "character": 17 }
            }
        }),
        "textDocument/references" => json!([]),
        _ => Value::Null,
    }))
    .await;
    let remote = gateway.addr();
    let root = ws.root();
    let edits = vec![
        (ws.path("src/shadow.rs"), SHADOW_PROPOSED.to_string()),
        (ws.path("src/shadow/retry.rs"), RETRY_PROPOSED.to_string()),
    ];
    let also_check = [ws.path("src/tools.rs")];

    let reports = diagnostics::validate_texts(remote, &root, &edits, &also_check)
        .await
        .expect("validation runs");
    assert_eq!(reports[0].warnings, 1);
    assert_eq!(reports[0].items[0].code.as_deref(), Some("unused_imports"));
}

#[tokio::test]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY pointing to a running Rust Analyzer gateway"]
async fn public_mcp_validation_accepts_a_nested_module_reexport() {
    let remote = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY to run this integration test")
        .parse::<SocketAddr>()
        .expect("PROD_CODE_LIVE_GATEWAY must be a socket address");
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/lib.rs", LIB),
        ("src/shadow.rs", SHADOW),
        ("src/tools.rs", TOOLS),
        ("src/compile_check.rs", COMPILE_CHECK),
        ("tests/slicing.rs", SLICING),
    ]);
    let root = ws.root();

    // Wait for rust-analyzer to finish loading the fixture; otherwise a cold workspace can
    // report only the unlinked-file hint and would not exercise stale-reference detection.
    let lib = ws.path("src/lib.rs");
    let mut seen = String::new();
    let mut loaded = false;
    for _ in 0..120 {
        match diagnostics::validate_text(remote, &root, &lib, BROKEN_LIB).await {
            Ok(report) => {
                seen = report.render();
                if report
                    .items
                    .iter()
                    .any(|item| item.code.as_deref() == Some("E0308"))
                {
                    loaded = true;
                    break;
                }
            }
            Err(error) => seen = format!("{error:#}"),
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert!(loaded, "rust-analyzer did not load the fixture:\n{seen}");

    let response = prod_code_mcp::handle_mcp_request(
        remote,
        &root,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "code_validate_edits",
                "arguments": {
                    "edits": [
                        { "path": "src/shadow.rs", "new_text": SHADOW_PROPOSED },
                        { "path": "src/shadow/retry.rs", "new_text": RETRY_PROPOSED }
                    ],
                    "also_check": [
                        "src/tools.rs",
                        "src/compile_check.rs",
                        "tests/slicing.rs"
                    ],
                    "compile": true
                }
            }
        }),
    )
    .await
    .expect("MCP request succeeds")
    .expect("tools/call produces a response");
    let result = &response["result"];
    let text = result["content"]
        .as_array()
        .expect("MCP content array")
        .iter()
        .filter_map(|item| item["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(result["isError"], Value::Bool(false), "{text}");
    assert!(
        text.starts_with("5 file(s) checked together: 0 error(s), 0 warning(s)"),
        "the valid public re-export must leave all edited and caller files clean:\n{text}"
    );
}
