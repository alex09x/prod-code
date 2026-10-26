/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::{net::SocketAddr, process::Command};

pub const CARGO: &str =
    "[package]\nname = \"preservation\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// One fixture per source/script case. Reuse it across apply/force modes, restoring
/// the original text first: the client's process-wide pools retain each distinct root.
pub fn workspace(source: &str) -> Workspace {
    Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/main.rs", source),
        ("untouched.txt", "sentinel\r\n"),
    ])
}

pub fn position(source: &str, needle: &str) -> (u32, u32) {
    prod_code_mcp::signature::position_at(source, source.find(needle).expect(needle)).unwrap()
}

pub fn locations(ws: &Workspace, source: &str, needles: &[&str]) -> Value {
    answers::locations(
        &ws.path("src/main.rs"),
        &needles
            .iter()
            .map(|n| position(source, n))
            .collect::<Vec<_>>(),
    )
}

/// Each response is tied to its declaration cursor, so querying a module word cannot
/// accidentally receive the destination binding's references.
pub async fn gateway(ws: &Workspace, source: &str, groups: &[(&str, &[&str])]) -> SocketAddr {
    let refs: Vec<_> = groups
        .iter()
        .map(|(decl, uses)| (position(source, decl), locations(ws, source, uses)))
        .collect();
    gateway_values(ws, source, refs).await
}

pub async fn gateway_values(
    ws: &Workspace,
    source: &str,
    refs: Vec<((u32, u32), Value)>,
) -> SocketAddr {
    let file = ws.path("src/main.rs");
    let target = position(source, "Tax");
    ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => {
            let pos = (
                params["position"]["line"].as_u64().unwrap() as u32 + 1,
                params["position"]["character"].as_u64().unwrap() as u32 + 1,
            );
            refs.iter()
                .find(|(p, _)| *p == pos)
                .map_or(json!([]), |(_, v)| v.clone())
        }
        "textDocument/definition" => answers::locations(&file, &[target]),
        "workspace/symbol" => json!([{
            "name": "Tax", "kind": 23,
            "location": { "uri": url::Url::from_file_path(&file).unwrap().to_string(),
                "range": { "start": { "line": target.0 - 1, "character": target.1 - 1 },
                    "end": { "line": target.0 - 1, "character": target.1 + 2 } } }
        }]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr()
}

pub async fn convert(
    ws: &Workspace,
    remote: SocketAddr,
    apply: bool,
    force: bool,
) -> anyhow::Result<prod_code_mcp::to_method::MadeMethod> {
    let source = ws.read("src/main.rs");
    let (l, c) = position(&source, "f(");
    prod_code_mcp::to_method::convert_to_method(
        remote,
        &ws.root(),
        &ws.path("src/main.rs"),
        l,
        c,
        apply,
        force,
    )
    .await
}

pub async fn move_receiver(
    ws: &Workspace,
    remote: SocketAddr,
    apply: bool,
    force: bool,
) -> anyhow::Result<prod_code_mcp::move_method::MovedMethod> {
    let source = ws.read("src/main.rs");
    let (l, c) = position(&source, "f(");
    prod_code_mcp::move_method::move_method(
        remote,
        &ws.root(),
        &ws.path("src/main.rs"),
        l,
        c,
        "tax",
        apply,
        force,
    )
    .await
}

pub async fn move_associated(
    ws: &Workspace,
    remote: SocketAddr,
    apply: bool,
    force: bool,
) -> anyhow::Result<prod_code_mcp::move_method::MovedMethod> {
    let source = ws.read("src/main.rs");
    let (l, c) = position(&source, "f(");
    prod_code_mcp::move_method::move_associated_function(
        remote,
        &ws.root(),
        &ws.path("src/main.rs"),
        l,
        c,
        "Tax",
        apply,
        force,
    )
    .await
}

pub fn untouched(ws: &Workspace, source: &str) {
    assert_eq!(ws.read("src/main.rs").as_bytes(), source.as_bytes());
    assert_eq!(ws.read("Cargo.toml"), CARGO);
    assert_eq!(ws.read("untouched.txt").as_bytes(), b"sentinel\r\n");
}

pub fn run(source: &str, label: &str) -> String {
    run_files(source, &[], label)
}

pub fn run_files(source: &str, files: &[(&str, &str)], label: &str) -> String {
    println!("{label}: source\n{source}");
    let dir = tempfile::tempdir().unwrap();
    for (path, text) in files {
        println!("{label}: {path}\n{text}");
        std::fs::write(dir.path().join(path), text).unwrap();
    }
    let input = dir.path().join("main.rs");
    let output = dir.path().join("program");
    std::fs::write(&input, source).unwrap();
    let compiler = Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
        .args(["--edition=2021", "-A", "warnings"])
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .output()
        .unwrap();
    println!(
        "{label}: rustc {}\n{}",
        compiler.status,
        String::from_utf8_lossy(&compiler.stderr)
    );
    assert!(compiler.status.success(), "rustc failed for {label}");
    let ran = Command::new(output).output().unwrap();
    let stdout = String::from_utf8(ran.stdout).unwrap();
    println!("{label}: runtime {}\n{stdout}", ran.status);
    assert!(
        ran.status.success(),
        "{}",
        String::from_utf8_lossy(&ran.stderr)
    );
    stdout
}
