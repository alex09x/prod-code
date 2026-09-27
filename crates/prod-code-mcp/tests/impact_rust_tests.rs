//! Public Rust impact selection regression: only runnable attributed tests are selectable.

use prod_code_mcp::impact::{self, CiRun, Symbol};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::process::Command;
use std::sync::Arc;

const CARGO_TOML: &str =
    "[package]\nname = \"impact-rust-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
const LIB: &str = "pub fn callee() -> u32 {\n    1\n}\n";
const TEST: &str = "fn test_helper() -> u32 {\n    impact_rust_fixture::callee()\n}\n\n#[test]\nfn actual_test() {\n    assert_eq!(test_helper(), 2);\n}\n\nfn unrelated_helper() {}\n";

fn sym(name: &str, file: &str, line: u32, col: u32) -> Symbol {
    Symbol {
        name: name.into(),
        file: file.into(),
        line,
        col,
    }
}

#[tokio::test]
async fn rust_impact_selects_the_attributed_test_and_executes_it_not_test_helpers() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", LIB),
        ("tests/impact.rs", TEST),
    ]);
    let root = ws.root();
    ws.write("src/lib.rs", &LIB.replace("1", "2"));
    let lib_uri = prod_code_protocol::path::file_uri(ws.path("src/lib.rs").as_path());
    let test_uri = prod_code_protocol::path::file_uri(ws.path("tests/impact.rs").as_path());
    let remote = ScriptedGateway::start_arc(Arc::new(move |method, params| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("callee", 12, 1, 3, 8),])
        }
        "textDocument/prepareCallHierarchy" => {
            let uri = params
                .pointer("/textDocument/uri")
                .and_then(|uri| uri.as_str())
                .unwrap_or("");
            let line = params
                .pointer("/position/line")
                .and_then(|line| line.as_u64());
            match (uri, line) {
                (uri, Some(0)) if uri == lib_uri => {
                    serde_json::json!([{ "name": "callee", "uri": lib_uri, "_id": "callee" }])
                }
                (uri, Some(0)) if uri == test_uri => serde_json::json!([
                    { "name": "test_helper", "uri": test_uri, "_id": "test_helper" }
                ]),
                (uri, Some(5)) if uri == test_uri => serde_json::json!([
                    { "name": "tests::actual_test", "uri": test_uri, "_id": "actual_test" }
                ]),
                _ => serde_json::json!([]),
            }
        }
        "callHierarchy/incomingCalls" => {
            match params.pointer("/item/_id").and_then(|id| id.as_str()) {
                Some("callee") => serde_json::json!([{
                    "from": {
                        "name": "test_helper",
                        "uri": test_uri,
                        "selectionRange": { "start": { "line": 0, "character": 3 } }
                    }
                }]),
                Some("test_helper") => serde_json::json!([{
                    "from": {
                        "name": "tests::actual_test",
                        "uri": test_uri,
                        "selectionRange": { "start": { "line": 5, "character": 3 } }
                    }
                }]),
                _ => serde_json::json!([]),
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 4)
        .await
        .expect("analysis runs");

    assert!(report.incomplete.is_empty(), "{:?}", report.incomplete);
    assert_eq!(report.changed, vec![sym("callee", "src/lib.rs", 1, 8)]);
    assert_eq!(
        report.callers,
        vec![sym("test_helper", "tests/impact.rs", 1, 4)]
    );
    assert_eq!(
        report.tests,
        vec![sym("tests::actual_test", "tests/impact.rs", 6, 4)]
    );
    let command = report.test_command.expect("the actual test is selectable");
    assert!(
        !command.iter().any(|arg| arg == "test_helper"),
        "{command:?}"
    );
    let output = Command::new(&command[0])
        .args(&command[1..])
        .current_dir(&root)
        .env("CARGO_TARGET_DIR", root.join("impact-target"))
        .output()
        .expect("the generated test command starts");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("actual_test"), "{stdout}");
    assert!(!stdout.contains("test_helper ... ok"), "{stdout}");
}

#[tokio::test]
async fn malformed_rust_test_evidence_runs_the_whole_suite() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", LIB)]);
    let root = ws.root();
    ws.write(
        "src/lib.rs",
        "pub fn callee() -> u32 {\n    2\n}\nconst UNTERMINATED: &str = \"#[test];\n",
    );
    let uri = prod_code_protocol::path::file_uri(ws.path("src/lib.rs").as_path());
    let remote = ScriptedGateway::start(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("callee", 12, 1, 3, 8),])
        }
        "textDocument/prepareCallHierarchy" => {
            serde_json::json!([{ "name": "callee", "uri": uri, "_id": "callee" }])
        }
        "callHierarchy/incomingCalls" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    })
    .await
    .addr();

    let report = impact::analyze(remote, &root, None, 4)
        .await
        .expect("analysis runs");

    assert_eq!(report.ci_decision().run, CiRun::WholeSuite);
    assert!(
        report
            .incomplete
            .iter()
            .any(|gap| gap.describe().contains("unterminated string")),
        "{:?}",
        report.incomplete
    );
}
