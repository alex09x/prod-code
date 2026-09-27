//! Public builder previews preserve verification failures and never write the checkout.
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

const SOURCE: &str =
    "#[derive(Debug)]\npub struct Settings {\n    pub name: String,\n    pub retries: u8,\n}\n";

#[derive(Clone, Copy)]
enum Verdict {
    Clean,
    Silent,
    Rejected,
    Blind,
}

fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"builder_demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", SOURCE),
    ])
}

async fn gateway(ws: &Workspace, verdict: Verdict) -> ScriptedGateway {
    let path = ws.path("src/lib.rs");
    let latest = Arc::new(Mutex::new(SOURCE.to_string()));
    ScriptedGateway::start(move |method, args| match method {
        "workspace/symbol" if args["query"] == "Settings" => json!([answers::symbol("Settings", 23, &path, 2, 12)]),
        "workspace/symbol" => json!([]),
        "textDocument/documentSymbol" => json!([{
            "name":"Settings", "kind":23,
            "range":{"start":{"line":0,"character":0},"end":{"line":4,"character":1}},
            "selectionRange":{"start":{"line":1,"character":11},"end":{"line":1,"character":19}},
            "children":[
                {"name":"name","kind":8,"range":{"start":{"line":2,"character":4},"end":{"line":2,"character":21}},"selectionRange":{"start":{"line":2,"character":8},"end":{"line":2,"character":12}}},
                {"name":"retries","kind":8,"range":{"start":{"line":3,"character":4},"end":{"line":3,"character":20}},"selectionRange":{"start":{"line":3,"character":8},"end":{"line":3,"character":15}}}
            ]
        }]),
        "textDocument/didOpen" => {
            if let Some(text) = args.pointer("/textDocument/text").and_then(Value::as_str) { *latest.lock().unwrap() = text.to_string(); }
            Value::Null
        }
        "textDocument/didChange" => {
            if let Some(text) = args.pointer("/contentChanges/0/text").and_then(Value::as_str) { *latest.lock().unwrap() = text.to_string(); }
            Value::Null
        }
        "textDocument/definition" => {
            if matches!(verdict, Verdict::Blind) { return Value::Null; }
            let text = latest.lock().unwrap();
            let line = args["position"]["line"].as_u64().unwrap() as usize;
            let col = args["position"]["character"].as_u64().unwrap() as usize;
            let name = text.lines().nth(line).unwrap().get(col..).unwrap();
            if name.starts_with("Settings;") { answers::locations(&path, &[(2, 12)]) } else { Value::Null }
        }
        "textDocument/diagnostic" => {
            let mut items = Vec::new();
            for (line, text) in latest.lock().unwrap().lines().enumerate() {
                for (needle, code, message) in [
                    ("__prod_code_missing_method", "E0599", "no method __prod_code_missing_method"),
                    ("self.retries.ok_or", "E0308", "mismatched types in builder"),
                ] {
                    if (needle.starts_with("__") && !matches!(verdict, Verdict::Silent))
                        || (needle.starts_with("self") && matches!(verdict, Verdict::Rejected)) {
                        if let Some(col) = text.find(needle) {
                            items.push(json!({"severity":1,"code":code,"message":message,
                                "range":{"start":{"line":line,"character":col},"end":{"line":line,"character":col+needle.len()}}}));
                        }
                    }
                }
            }
            json!({"kind":"full","items":items})
        }
        _ => Value::Null,
    }).await
}

async fn cli(ws: &Workspace, remote: SocketAddr, args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .current_dir(ws.root())
        .output()
        .await
        .expect("run CLI")
}
fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}
fn detail(out: &Output) -> String {
    format!(
        "{:?}\n{}\n{}",
        out.status.code(),
        stdout(out),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn compile_report(report: &str, name: &str) {
    let code = report
        .split_once("```rust\n")
        .expect("Rust preview")
        .1
        .split_once("\n```")
        .unwrap()
        .0;
    let dir = tempfile::tempdir().unwrap();
    let program = format!(
        "{SOURCE}\n{code}\nfn main() {{\nlet x = {name}::new().retries(3).name(\"demo\".to_string()).build().unwrap();\nprintln!(\"{{}} {{}}\", x.name, x.retries);\nprintln!(\"{{}}\", {name}::new().name(String::new()).build().unwrap_err().field());\n}}\n"
    );
    std::fs::write(dir.path().join("main.rs"), program).unwrap();
    let built = Command::new("rustc")
        .current_dir(dir.path())
        .args([
            "--edition",
            "2021",
            "-D",
            "warnings",
            "main.rs",
            "-o",
            "program",
        ])
        .output()
        .unwrap();
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let ran = Command::new(dir.path().join("program")).output().unwrap();
    assert!(ran.status.success());
    assert_eq!(ran.stdout, b"demo 3\nretries\n");
}

#[tokio::test]
async fn cli_builders_compile_and_leave_the_source_unchanged() {
    let ws = workspace();
    let gw = gateway(&ws, Verdict::Clean).await;
    let out = cli(&ws, gw.addr(), &["fixture", "Settings", "--builder"]).await;
    assert!(out.status.success(), "{}", detail(&out));
    assert!(
        stdout(&out).contains("verified: the analyzer checked"),
        "{}",
        detail(&out)
    );
    compile_report(&stdout(&out), "SettingsBuilder");
    let custom = cli(
        &ws,
        gw.addr(),
        &[
            "fixture",
            "Settings",
            "--builder",
            "--builder-name",
            "Setup",
            "--path",
            "src/lib.rs",
            "--no-verify",
        ],
    )
    .await;
    assert!(custom.status.success(), "{}", detail(&custom));
    assert!(stdout(&custom).contains("not verified:"));
    compile_report(&stdout(&custom), "Setup");
    assert_eq!(ws.read("src/lib.rs"), SOURCE);
}

#[tokio::test]
async fn cli_builder_verification_failure_is_unsuccessful() {
    let ws = workspace();
    for (verdict, expected) in [
        (Verdict::Silent, "did not report a deliberate error"),
        (Verdict::Rejected, "mismatched types"),
        (Verdict::Blind, "did not resolve"),
    ] {
        let gw = gateway(&ws, verdict).await;
        let out = cli(&ws, gw.addr(), &["fixture", "Settings", "--builder"]).await;
        assert_eq!(out.status.code(), Some(1), "{}", detail(&out));
        assert!(stdout(&out).contains(expected), "{}", detail(&out));
        assert_eq!(ws.read("src/lib.rs"), SOURCE);
    }
}

#[tokio::test]
async fn cli_builder_arguments_are_explicit_and_value_mode_is_preserved() {
    let ws = workspace();
    let gw = gateway(&ws, Verdict::Clean).await;
    for args in [
        vec!["fixture", "Settings", "--builder-name", "Setup"],
        vec!["fixture", "Settings", "--builder", "--depth", "3"],
    ] {
        let out = cli(&ws, gw.addr(), &args).await;
        assert_eq!(out.status.code(), Some(2), "{}", detail(&out));
    }
    let value = cli(
        &ws,
        gw.addr(),
        &["fixture", "Settings", "--depth", "1", "--no-verify"],
    )
    .await;
    assert!(value.status.success(), "{}", detail(&value));
    assert!(stdout(&value).contains("fixture for `Settings`"));
    assert!(!stdout(&value).contains("SettingsBuilder"));
    assert_eq!(ws.read("src/lib.rs"), SOURCE);
}

fn tool_text(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| {
            let prod_code_mcp::protocol::McpContentItem::Text { text } = item;
            text.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn mcp_builder_preview_propagates_verification_and_mode_arguments() {
    let ws = workspace();
    for (verdict, expected_error) in [
        (Verdict::Clean, false),
        (Verdict::Silent, true),
        (Verdict::Rejected, true),
        (Verdict::Blind, true),
    ] {
        let gw = gateway(&ws, verdict).await;
        let report = prod_code_mcp::tools::execute_tool(
            gw.addr(),
            &ws.root(),
            "code_generate_fixture",
            json!({"symbol":"Settings","builder":true}),
        )
        .await
        .unwrap();
        assert_eq!(report.is_error, expected_error, "{}", tool_text(&report));
        if !expected_error {
            compile_report(&tool_text(&report), "SettingsBuilder");
        }
        assert_eq!(ws.read("src/lib.rs"), SOURCE);
    }
    let gw = gateway(&ws, Verdict::Silent).await;
    let preview = prod_code_mcp::tools::execute_tool(gw.addr(), &ws.root(), "code_generate_fixture", json!({"symbol":"Settings","builder":true,"builder_name":"Setup","verify":false,"path":"src/lib.rs"})).await.unwrap();
    assert!(!preview.is_error, "{}", tool_text(&preview));
    assert!(tool_text(&preview).contains("not verified:"));
    compile_report(&tool_text(&preview), "Setup");
    for args in [
        json!({"symbol":"Settings","builder_name":"Setup"}),
        json!({"symbol":"Settings","builder":true,"depth":2}),
        json!({"symbol":"Settings","builder":"yes"}),
        json!({"symbol":"Settings","builder":true,"builder_name":7}),
        json!({"symbol":"Settings","builder":true,"verify":"false"}),
    ] {
        assert!(
            prod_code_mcp::tools::execute_tool(
                gw.addr(),
                &ws.root(),
                "code_generate_fixture",
                args.clone()
            )
            .await
            .is_err(),
            "{args}"
        );
    }
    let value = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol":"Settings","verify":false,"depth":1}),
    )
    .await
    .unwrap();
    assert!(!value.is_error);
    assert!(tool_text(&value).contains("fixture for `Settings`"));
    assert!(!tool_text(&value).contains("SettingsBuilder"));
    assert_eq!(ws.read("src/lib.rs"), SOURCE);
}

/// Explicitly run with a gateway containing the real Rust analyzer. It cannot pass by skipping.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires PROD_CODE_LIVE_GATEWAY with a real Rust analyzer"]
async fn real_analyzer_verifies_builder_previews_through_cli_and_mcp() {
    let remote: SocketAddr = std::env::var("PROD_CODE_LIVE_GATEWAY")
        .expect("set PROD_CODE_LIVE_GATEWAY")
        .parse()
        .expect("socket address");
    let ws = workspace();
    // The real analyzer may still be loading the fresh Cargo project. Keep the last answer
    // and require an affirmative verdict; silence or exhausted retries always fail.
    let mut last = String::new();
    let mut verified = false;
    for _ in 0..30 {
        let out = cli(&ws, remote, &["fixture", "Settings", "--builder"]).await;
        last = detail(&out);
        if out.status.success() && stdout(&out).contains("verified: the analyzer checked") {
            compile_report(&stdout(&out), "SettingsBuilder");
            verified = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    assert!(verified, "{last}");
    let report = prod_code_mcp::tools::execute_tool(
        remote,
        &ws.root(),
        "code_generate_fixture",
        json!({"symbol":"Settings","builder":true,"builder_name":"Setup"}),
    )
    .await
    .unwrap();
    assert!(!report.is_error, "{}", tool_text(&report));
    assert!(
        tool_text(&report).contains("verified: the analyzer checked"),
        "{}",
        tool_text(&report)
    );
    compile_report(&tool_text(&report), "Setup");
    assert_eq!(ws.read("src/lib.rs"), SOURCE);
}
