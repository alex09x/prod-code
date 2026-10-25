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
async fn hover_by_symbol_resolves_file_hint_and_renders_text() {
    let ws = workspace();
    let path = ws.path("src/lib.rs");
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => {
            assert_eq!(params["query"], "sample");
            json!([prod_code_testkit::answers::symbol(
                "sample", 12, &path, 1, 8
            )])
        }
        "textDocument/hover" => {
            assert_eq!(params["position"], json!({"line":0,"character":7}));
            json!({"contents":{"value":"fn sample() — λ"}})
        }
        _ => Value::Null,
    })
    .await;
    let output = cli(
        &ws,
        &gateway,
        &["hover", "--symbol", "sample", "src/lib.rs"],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "fn sample() — λ\n");
}

#[tokio::test]
async fn report_reads_body_from_stdin_and_preserves_private_reference() {
    use tokio::io::AsyncWriteExt;
    let ws = workspace();
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("-r")
        .arg(gateway.addr().to_string())
        .args([
            "report-issue",
            "--title",
            "A neutral lookup fails",
            "--body-file",
            "-",
            "--dry-run",
            "--allow-private-names",
            "--private-ref",
            "sample-record",
        ])
        .env("HOME", ws.root())
        .current_dir(ws.root())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let body = "Ran a neutral lookup command. It returned an empty result. Expected the matching declaration.";
    child
        .stdin
        .take()
        .unwrap()
        .write_all(body.as_bytes())
        .await
        .unwrap();
    let output = child.wait_with_output().await.unwrap();
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let text = stdout(&output);
    let prefix = format!(
        "Dry run, nothing was filed. The issue would be:\n\n# A neutral lookup fails\nLabels: bug\n\n{body}\n\n"
    );
    assert_eq!(&text[..prefix.len()], prefix);
    assert!(text.contains(
        "Private details: report `sample-record`.\n_Filed with `prod-code report-issue`._\n"
    ));
}

#[tokio::test]
async fn lsp_errors_preserve_message_and_unknown_fallback() {
    let ws = workspace();
    for (error, expected) in [
        (
            json!({"code":-32603,"message":"sample refused"}),
            "sample refused",
        ),
        (json!({"code":-32603}), "unknown error"),
    ] {
        let gateway = ScriptedGateway::start(move |method, _| {
            if method == "textDocument/hover" {
                json!({prod_code_testkit::LSP_ERROR:error})
            } else {
                Value::Null
            }
        })
        .await;
        let absolute = ws.path("src/lib.rs");
        let output = cli(
            &ws,
            &gateway,
            &["hover", absolute.to_str().unwrap(), "1", "1"],
        )
        .await;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(stdout(&output), "");
        assert!(
            stderr(&output).contains(&format!("Error: textDocument/hover failed: {expected}\n")),
            "{}",
            stderr(&output)
        );
        assert_eq!(gateway.calls(), 1);
    }
}

fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
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
        .env("PROD_CODE_TIMING", "1")
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
async fn hover_contents_table() {
    let ws = workspace();
    for (contents, expected) in [
        (
            json!({"value":"λ documentation"}),
            Some("λ documentation\n"),
        ),
        (
            json!(["first", {"language":"rust","value":"fn sample()"}, "", {"bad":true}, {"value":" \n "}]),
            Some("first\nfn sample()\n"),
        ),
        (json!("plain text"), Some("plain text\n")),
        (json!(" \r\n "), None),
        (json!({"value":""}), None),
        (json!([{"value":"  "}, 42]), None),
        (Value::Null, None),
    ] {
        let gateway = ScriptedGateway::start(move |method, params| {
            if method == "textDocument/hover" {
                assert_eq!(params["position"], json!({"line":0,"character":0}));
                json!({"contents":contents})
            } else {
                Value::Null
            }
        })
        .await;
        let output = cli(&ws, &gateway, &["hover", "src/lib.rs", "0", "0"]).await;
        match expected {
            Some(text) => {
                assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
                assert_eq!(stdout(&output), text);
            }
            None => {
                assert_eq!(output.status.code(), Some(1));
                assert_eq!(stdout(&output), "");
                assert!(
                    stderr(&output)
                        .contains("Error: no hover information found at src/lib.rs:0:0\n"),
                    "{}",
                    stderr(&output)
                );
            }
        }
        assert_eq!(gateway.calls(), 1);
    }
}

#[tokio::test]
async fn definition_locations_objects_and_refusals() {
    let ws = workspace();
    let uri = url::Url::from_file_path(ws.path("src/lib.rs"))
        .unwrap()
        .to_string();
    for (answer, expected) in [
        (
            json!([{"uri":uri,"range":{"start":{"line":2,"character":4}}}]),
            Some(format!("📍 Definition: {uri}:3:5\n")),
        ),
        (
            json!([{"targetUri":uri,"targetSelectionRange":{"start":{"line":0,"character":0}}}]),
            Some(format!("📍 Definition: {uri}:1:1\n")),
        ),
        (
            json!({"uri":uri,"range":{"start":{"line":1,"character":2}}}),
            Some(format!("📍 Definition: {uri}:2:3\n")),
        ),
        (
            json!({"uri":uri}),
            Some(format!("📍 Definition: {uri}:1:1\n")),
        ),
        (json!([]), None),
        (json!({}), None),
        (Value::Null, None),
    ] {
        let gateway = ScriptedGateway::start(move |method, _| {
            if method == "textDocument/definition" {
                answer.clone()
            } else {
                Value::Null
            }
        })
        .await;
        let output = cli(&ws, &gateway, &["def", "src/lib.rs", "1", "1", "--no-body"]).await;
        if let Some(text) = expected {
            assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
            assert_eq!(stdout(&output), text);
        } else {
            assert_eq!(output.status.code(), Some(1));
            assert!(
                stderr(&output).contains("Error: no definition found at src/lib.rs:1:1\n"),
                "{}",
                stderr(&output)
            );
        }
    }
}

#[tokio::test]
async fn implementation_answer_shapes_and_default_positions() {
    let ws = workspace();
    let uri = url::Url::from_file_path(ws.path("src/lib.rs"))
        .unwrap()
        .to_string();
    for (answer, expected) in [
        (
            json!({"uri":uri,"range":{"start":{"line":1,"character":3}}}),
            format!("Found 1 implementation(s):\n  • {uri}:2:4\n"),
        ),
        (
            json!([{"uri":uri},{}]),
            format!("Found 2 implementation(s):\n  • {uri}:1:1\n  • :1:1\n"),
        ),
        (Value::Null, "No implementations found.\n".into()),
    ] {
        let gateway = ScriptedGateway::start(move |method, _| {
            if method == "textDocument/implementation" {
                answer.clone()
            } else {
                Value::Null
            }
        })
        .await;
        let output = cli(&ws, &gateway, &["impls", "src/lib.rs", "1", "1"]).await;
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        assert_eq!(stdout(&output), expected);
    }
}

#[tokio::test]
async fn assists_list_preserves_subtypes_and_missing_fields() {
    let ws = workspace();
    let gateway = ScriptedGateway::start(|method,params| {
        if method == "prodCode/assists" {
            assert_eq!(params["range"], json!({"start":{"line":0,"character":0},"end":{"line":0,"character":3}}));
            json!([{"id":"extract","kind":"refactor","label":"Extract","subtype":7},{"id":"inline","kind":"quickfix","label":"Inline"},{}])
        } else {Value::Null}
    }).await;
    let output = cli(
        &ws,
        &gateway,
        &["assists", "src/lib.rs", "1", "1", "--to", "1:4"],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        stdout(&output),
        "extract --subtype 7  [refactor]  Extract\ninline  [quickfix]  Inline\n?  []  \n"
    );
}

#[tokio::test]
async fn report_dry_run_sources_and_input_errors() {
    let ws = workspace();
    ws.write("body.txt","Ran a neutral lookup command. It returned an empty result. Expected the matching declaration.");
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    for extra in [
        vec![
            "--body",
            "Ran a neutral lookup command. It returned an empty result. Expected the matching declaration.",
        ],
        vec!["--body-file", "body.txt"],
    ] {
        let mut args = vec![
            "report-issue",
            "--title",
            "A neutral lookup fails",
            "--dry-run",
            "--allow-private-names",
            "--label",
            "client",
        ];
        args.extend(extra);
        let output = cli(&ws, &gateway, &args).await;
        assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
        let text = stdout(&output);
        assert!(text.contains("A neutral lookup fails"), "{text}");
        assert!(text.contains("Ran a neutral lookup command. It returned an empty result. Expected the matching declaration."),"{text}");
        assert!(text.contains("client"), "{text}");
    }
    let output = cli(
        &ws,
        &gateway,
        &["report-issue", "--title", "Missing body", "--dry-run"],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("give the report a --body or a --body-file"),
        "{}",
        stderr(&output)
    );
    let output = cli(
        &ws,
        &gateway,
        &[
            "report-issue",
            "--title",
            "Missing body file",
            "--body-file",
            "absent.txt",
            "--dry-run",
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("failed to read absent.txt"),
        "{}",
        stderr(&output)
    );
}
