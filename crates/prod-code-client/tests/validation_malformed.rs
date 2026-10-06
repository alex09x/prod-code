/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Required diagnostic evidence must not be mistaken for successful validation.
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::process::Output;

const ORIGINAL: &str = "pub fn value() -> u32 { 1 }\n";
const PROPOSAL: &str = "pub fn value() -> u32 { 2 }\n";

fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"diagnostic_evidence\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", ORIGINAL),
        ("src/other.rs", ORIGINAL),
        ("proposal.rs", PROPOSAL),
    ])
}

async fn gateway(answer: Value) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/diagnostic" => answer.clone(),
        "textDocument/documentSymbol" => json!([]),
        _ => Value::Null,
    })
    .await
}

async fn cli(ws: &Workspace, remote: SocketAddr, multi: bool) -> Output {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    cmd.arg("--remote").arg(remote.to_string()).args([
        "validate",
        "src/lib.rs",
        "--from",
        "proposal.rs",
        "--json",
    ]);
    if multi {
        cmd.args(["--with", "src/other.rs=proposal.rs"]);
    }
    cmd.env("PROD_CODE_REMOTE", remote.to_string())
        .current_dir(ws.root())
        .output()
        .await
        .unwrap()
}

fn malformed_reports() -> Vec<Value> {
    let good = json!({"severity":1,"message":"a real error","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}});
    let mut cases = vec![
        Value::Null,
        json!({}),
        json!({"items":{}}),
        json!({"kind":"unchanged","resultId":"unknown"}),
        json!({"items":[null]}),
    ];
    for (at, value) in [
        ("line", json!(u32::MAX)),
        ("line", json!(1u64 << 32)),
        ("character", json!(-1)),
        ("character", json!(1.5)),
    ] {
        let mut diagnostic = good.clone();
        diagnostic["range"]["start"][at] = value;
        cases.push(json!({"kind":"full","items":[diagnostic]}));
    }
    let mut reversed = good.clone();
    reversed["range"]["start"]["character"] = json!(2);
    cases.push(json!({"kind":"full","items":[reversed]}));
    let mut missing = good.clone();
    missing.as_object_mut().unwrap().remove("message");
    cases.push(json!({"kind":"full","items":[missing]}));
    cases
}

#[tokio::test]
async fn cli_validation_refuses_malformed_reports_without_panicking_or_writing() {
    let ws = workspace();
    let mut wrong = Vec::new();
    for answer in malformed_reports() {
        let gw = gateway(answer.clone()).await;
        for multi in [false, true] {
            let out = cli(&ws, gw.addr(), multi).await;
            let detail = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            if out.status.code() != Some(1)
                || !detail.contains("invalid-diagnostics")
                || detail.contains("panicked at")
            {
                wrong.push(format!(
                    "multi={multi}, {answer}: {:?}\n{detail}",
                    out.status.code()
                ));
            }
            assert_eq!(ws.read("src/lib.rs"), ORIGINAL);
            assert_eq!(ws.read("src/other.rs"), ORIGINAL);
            assert_eq!(ws.read("proposal.rs"), PROPOSAL);
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn mcp_validation_refuses_malformed_reports_even_when_the_baseline_is_identical() {
    let ws = workspace();
    let mut wrong = Vec::new();
    // Coordinate overflow is also exercised above through a subprocess so a base panic cannot
    // abort this matrix before it records false-success cases in both public tools.
    for answer in [
        Value::Null,
        json!({"items":{}}),
        json!({"kind":"unchanged","resultId":"unknown"}),
    ] {
        let gw = gateway(answer.clone()).await;
        for (tool, args) in [
            (
                "code_validate_edit",
                json!({"path":"src/lib.rs","new_text":PROPOSAL}),
            ),
            (
                "code_validate_edits",
                json!({"edits":[{"path":"src/lib.rs","new_text":PROPOSAL},{"path":"src/other.rs","new_text":PROPOSAL}]}),
            ),
        ] {
            let report = prod_code_mcp::tools::execute_tool(gw.addr(), &ws.root(), tool, args)
                .await
                .unwrap();
            let text = report
                .content
                .iter()
                .map(|c| {
                    let prod_code_mcp::protocol::McpContentItem::Text { text } = c;
                    text.as_str()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !report.is_error || !text.contains("invalid-diagnostics") {
                wrong.push(format!("{tool}, {answer}: {text}"));
            }
            assert_eq!(ws.read("src/lib.rs"), ORIGINAL);
            assert_eq!(ws.read("src/other.rs"), ORIGINAL);
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[tokio::test]
async fn complete_empty_diagnostic_reports_still_validate() {
    let ws = workspace();
    let gw = gateway(answers::no_diagnostics()).await;
    for multi in [false, true] {
        let out = cli(&ws, gw.addr(), multi).await;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let report = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_validate_edit",
        json!({"path":"src/lib.rs","new_text":PROPOSAL}),
    )
    .await
    .unwrap();
    assert!(!report.is_error);
    assert_eq!(ws.read("src/lib.rs"), ORIGINAL);
}
