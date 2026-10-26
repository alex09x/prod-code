/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Query commands that go to the analyzer through the scripted gateway: `--symbol` forms,
//! a file named from a subdirectory, an unlimited outline, and a proposed diff.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::process::Output;
use tokio::io::AsyncWriteExt;

const CARGO: &str = "[package]\nname = \"scratch\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

const LIB: &str = "pub fn parse(text: &str) -> u32 {\n    text.trim().len() as u32\n}\n\npub fn label() -> String {\n    String::new()\n}\n";

const PATCH: &str = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -5,3 +5,3 @@\n pub fn label() -> String {\n-    String::new()\n+    String::from(\"x\")\n }\n";

async fn cli_in(dir: &Path, remote: SocketAddr, args: &[&str]) -> Output {
    let home = tempfile::Builder::new()
        .prefix("dispatchhome")
        .tempdir()
        .expect("home");
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", home.path())
        .current_dir(dir)
        .output()
        .await
        .expect("run prod-code")
}

async fn piped(dir: &Path, remote: SocketAddr, args: &[&str], input: &str) -> Output {
    let home = tempfile::Builder::new()
        .prefix("dispatchhome")
        .tempdir()
        .expect("home");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(remote.to_string())
        .args(args)
        .env("PROD_CODE_REMOTE", remote.to_string())
        .env("HOME", home.path())
        .current_dir(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn prod-code");
    let mut stdin = child.stdin.take().expect("stdin");
    stdin
        .write_all(input.as_bytes())
        .await
        .expect("write stdin");
    drop(stdin);
    child.wait_with_output().await.expect("run prod-code")
}

fn succeeded(out: &Output) -> String {
    assert!(out.status.success(), "{:?}: {}", out.status, stderr(out));
    stdout(out)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[tokio::test]
async fn symbol_and_outline_queries() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    let lib = ws.path("src/lib.rs");
    let gateway = {
        let lib = lib.clone();
        ScriptedGateway::start(move |method, _| match method {
            "workspace/symbol" => json!([answers::symbol("parse", 12, &lib, 1, 8)]),
            "textDocument/hover" => answers::hover("```rust\nfn parse(text: &str) -> u32\n```"),
            "textDocument/implementation" => answers::locations(&lib, &[(5, 8)]),
            "textDocument/documentSymbol" => json!([
                answers::document_symbol("parse", 12, 1, 3, 8),
                answers::document_symbol("label", 12, 5, 7, 8),
            ]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        })
        .await
    };
    let patch_dir = tempfile::Builder::new()
        .prefix("patch")
        .tempdir()
        .expect("patch dir");
    let patch = patch_dir.path().join("change.patch");
    std::fs::write(&patch, PATCH).expect("write patch");
    let patch = patch.to_string_lossy().into_owned();
    let root = ws.root();
    let src = ws.path("src");
    let stdin_patch = std::fs::read_to_string(&patch).expect("read patch");
    let addr = gateway.addr();
    macro_rules! at {
        ($dir:expr, $args:expr) => {
            cli_in($dir, addr, $args)
        };
    }

    let out = at!(&root, &["hover", "--symbol", "parse"]).await;
    assert!(succeeded(&out).contains("fn parse(text: &str) -> u32"));

    let text = succeeded(&at!(&root, &["impls", "--symbol", "parse"]).await);
    assert!(text.contains("Found 1 implementation(s):"), "{text}");
    assert!(
        text.contains("src/lib.rs:5:8  `pub fn label() -> String {`"),
        "{text}"
    );

    // A file named relative to the checkout from inside one of its directories.
    let text = succeeded(&at!(&src, &["symbols", "src/lib.rs"]).await);
    assert_eq!(
        text,
        format!(
            "Outline for {}:\n  [Function] parse (line 1)\n  [Function] label (line 5)\n",
            lib.display()
        )
    );

    let text = succeeded(&at!(&root, &["outline", "src/lib.rs", "--max-bytes", "0"]).await);
    assert_eq!(
        text,
        "Outline for src/lib.rs:\n  [Function] parse (line 1)\n  [Function] label (line 5)\n"
    );

    let checked = "1 file(s) checked together: 0 error(s), 0 warning(s)\nsrc/lib.rs: 0 error(s), 0 warning(s)\n";
    assert_eq!(
        succeeded(&at!(&root, &["validate", "--diff", &patch]).await),
        checked
    );
    assert_eq!(
        succeeded(
            &piped(
                &root,
                gateway.addr(),
                &["validate", "--diff", "-"],
                &stdin_patch
            )
            .await
        ),
        checked
    );

    for (flag, announced) in [
        (
            "--compile",
            "Running the remote compiler check for the proposed diff...\n",
        ),
        (
            "--borrow-check",
            "Running the remote compiler check for the proposed diff (with borrow-checker proof)...\n",
        ),
    ] {
        // The scripted gateway answers shadow runs with exit 0, so the check passes and the
        // report names the exact command it ran.
        let out = at!(&root, &["validate", "--diff", &patch, flag]).await;
        let (stdout, err) = (String::from_utf8_lossy(&out.stdout), stderr(&out));
        assert_eq!(
            out.status.code(),
            Some(0),
            "{flag}\nstdout:\n{stdout}\nstderr:\n{err}"
        );
        assert!(err.contains(announced), "{flag}: {err}");
        assert!(
            stdout.contains("argv=[\"cargo\",\"check\"") && stdout.contains("exit=0"),
            "{flag}: {stdout}"
        );
    }
}

#[tokio::test]
async fn cluster_report_lists_the_node_that_was_asked() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    let node = gateway.addr().to_string();

    let out = cli_in(&ws.root(), gateway.addr(), &["cluster", "--json"]).await;
    let report: Value = serde_json::from_str(&succeeded(&out)).expect("the report is JSON");
    assert_eq!(report["nodes"].as_array().map(Vec::len), Some(1));
    assert_eq!(report["nodes"][0]["remote"], node.as_str());
    assert_eq!(report["nodes"][0]["up"], true);
    assert_eq!(report["nodes"][0]["healthy"], true);
    assert_eq!(report["nodes"][0]["cpu_count"], 4);
    assert_eq!(report["placed_on"], Value::Null);
    assert_eq!(report["rebalanced"], Value::Null);

    let out = cli_in(&ws.root(), gateway.addr(), &["cluster"]).await;
    let text = succeeded(&out);
    assert!(
        text.starts_with("\u{26a1} prod-code cluster (1 node(s))\n"),
        "{text}"
    );
    assert!(text.contains(&format!("{node}        UP ")), "{text}");
    assert!(text.contains("Engine needed:       rust\n"), "{text}");
    assert!(text.contains("Placed on:           (not yet)\n"), "{text}");
}

#[tokio::test]
async fn resolve_reports_internal_and_external_domains() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO), ("src/lib.rs", LIB)]);
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    let root = ws.root();

    let out = cli_in(
        &root,
        gateway.addr(),
        &["resolve", " Shop.code.internal ", "--json"],
    )
    .await;
    let report: Value = serde_json::from_str(&succeeded(&out)).expect("the report is JSON");
    assert_eq!(report["query"], " Shop.code.internal ");
    assert_eq!(report["domain"], "Shop.code.internal");
    assert_eq!(report["is_internal"], true);
    assert_eq!(report["project"], "shop");
    assert_eq!(
        report["srv_records"].as_array().map(Vec::len),
        report["discovered_nodes"].as_u64().map(|n| n as usize)
    );

    let out = cli_in(&root, gateway.addr(), &["resolve", "Shop.code.internal"]).await;
    let text = succeeded(&out);
    assert!(
        text.starts_with("\u{26a1} prod-code resolve: Shop.code.internal\n"),
        "{text}"
    );
    assert!(
        text.contains("Type:               Internal cluster domain (*.code.internal)\n"),
        "{text}"
    );
    assert!(text.contains("Project:            shop"), "{text}");

    let out = cli_in(&root, gateway.addr(), &["resolve", "localhost", "--json"]).await;
    let report: Value = serde_json::from_str(&succeeded(&out)).expect("the report is JSON");
    assert_eq!(report["is_internal"], false);
    assert_eq!(report["project"], Value::Null);
    let endpoints = report["endpoints"].as_array().expect("endpoints");
    assert!(!endpoints.is_empty());
    assert!(
        endpoints
            .iter()
            .all(|e| e.as_str().unwrap().ends_with(":9400")),
        "{endpoints:?}"
    );

    let out = cli_in(&root, gateway.addr(), &["resolve", "localhost"]).await;
    let text = succeeded(&out);
    assert!(
        text.contains("Type:               Standard host / external address\n"),
        "{text}"
    );
    assert!(text.contains("Endpoints ("), "{text}");
    assert!(text.contains(":9400\n"), "{text}");
}
