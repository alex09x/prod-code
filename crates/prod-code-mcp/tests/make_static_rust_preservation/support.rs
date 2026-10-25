/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::make_static::MadeStatic;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::{net::SocketAddr, process::Command};

pub const CARGO: &str =
    "[package]\nname = \"preservation\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

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

pub fn references(ws: &Workspace, source: &str, needles: &[&str]) -> Value {
    answers::locations(
        &ws.path("src/main.rs"),
        &needles
            .iter()
            .map(|n| position(source, n))
            .collect::<Vec<_>>(),
    )
}

pub async fn gateway(refs: Value) -> SocketAddr {
    ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr()
}

pub async fn change(
    ws: &Workspace,
    remote: SocketAddr,
    apply: bool,
    force: bool,
) -> anyhow::Result<MadeStatic> {
    let source = ws.read("src/main.rs");
    let (line, col) = position(&source, "consume");
    prod_code_mcp::make_static::make_static(
        remote,
        &ws.root(),
        &ws.path("src/main.rs"),
        line,
        col,
        apply,
        force,
    )
    .await
}

pub fn untouched(ws: &Workspace, source: &str) {
    for (path, expected) in [
        ("Cargo.toml", CARGO),
        ("src/main.rs", source),
        ("untouched.txt", "sentinel\r\n"),
    ] {
        assert_eq!(
            std::fs::read(ws.path(path)).unwrap(),
            expected.as_bytes(),
            "changed bytes in {path}"
        );
    }
}

/// This invokes the real compiler and executable on the build node running the test.
pub fn run(source: &str, label: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
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
        "{label}: rustc --edition=2021 -A warnings main.rs -o program: {}\n{}",
        compiler.status,
        String::from_utf8_lossy(&compiler.stderr)
    );
    assert!(compiler.status.success(), "rustc failed for {label}");
    let ran = Command::new(output).output().unwrap();
    let stdout = String::from_utf8(ran.stdout).unwrap();
    println!(
        "{label}: runtime {}\n{stdout}{}",
        ran.status,
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(ran.status.success());
    stdout
}

pub fn owning_source(receiver: &str, init: &str, call: &str) -> String {
    format!(
        "struct S;\nimpl Drop for S {{ fn drop(&mut self) {{ println!(\"drop\"); }} }}\nimpl S {{\n    fn consume({receiver}) {{ println!(\"method\"); }}\n}}\nfn main() {{\n    let value = {init};\n    {call};\n    println!(\"after\");\n}}\n"
    )
}
