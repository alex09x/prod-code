/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};
use std::process::Output;

#[tokio::test]
async fn impact_ci_skips_execution_when_no_function_changed() {
    let ws = workspace();
    let gateway = ScriptedGateway::start(|_, _| json!([])).await;
    let output = cli(&ws, &gateway, &["impact", "--ci", "--json"]).await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let (report, decision) = text.split_once("\n[prod-code impact --ci] ").unwrap();
    assert_eq!(decision, "no function changed\n");
    let report: Value = serde_json::from_str(report).unwrap();
    assert_eq!(report["changed"], json!([]));
    assert_eq!(gateway.calls(), 0);
}

fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname=\"sample\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        ),
        ("src/lib.rs", "pub fn sample() {}\r\n"),
    ])
}
async fn cli(ws: &Workspace, gateway: &ScriptedGateway, args: &[&str]) -> Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("-r")
        .arg(gateway.addr().to_string())
        .args(args)
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .output()
        .await
        .unwrap()
}
fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[tokio::test]
async fn dataflow_slice_without_declaration_refuses_absolute_target() {
    let ws = workspace();
    let gateway = ScriptedGateway::start(|_, _| json!([])).await;
    let target = ws.path("src/lib.rs");
    let output = cli(
        &ws,
        &gateway,
        &[
            "slice",
            target.to_str().unwrap(),
            "--line",
            "1",
            "--dataflow",
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(stdout(&output), "");
    assert!(
        stderr(&output).contains("Error: no declaration at src/lib.rs:1\n"),
        "{}",
        stderr(&output)
    );
}
#[tokio::test]
async fn outline_refuses_empty_file_and_directory_answers() {
    let ws = workspace();
    let gateway = ScriptedGateway::start(|_, _| json!([])).await;
    for path in ["src/lib.rs", "src"] {
        let output = cli(&ws, &gateway, &["outline", path]).await;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(stdout(&output), "");
        assert!(
            stderr(&output).contains(&format!("no outline symbols found for {path}\n")),
            "{}",
            stderr(&output)
        );
    }
}
#[tokio::test]
async fn source_without_context_preserves_unicode_and_crlf() {
    let ws = workspace();
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "prod-code/readFile" {
            json!("λ first\r\nsecond\r\n")
        } else {
            Value::Null
        }
    })
    .await;
    let output = cli(&ws, &gateway, &["source", "/virtual/source.rs"]).await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "λ first\r\nsecond\r\n");
}
#[tokio::test]
async fn prune_empty_checkout_patch_and_json() {
    let ws = Workspace::new(&[(
        "Cargo.toml",
        "[package]\nname=\"sample\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )]);
    let gateway = ScriptedGateway::start(|_, _| json!([])).await;
    let output = cli(&ws, &gateway, &["prune", "--patch"]).await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "nothing to patch; 0 orphans found\n");
    let output = cli(&ws, &gateway, &["prune", "--json"]).await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let report: Value = serde_json::from_str(&stdout(&output)).unwrap();
    assert_eq!(report["diagnostics"], json!([]));
}
