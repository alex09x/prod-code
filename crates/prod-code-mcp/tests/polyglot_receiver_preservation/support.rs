/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::to_method::{convert_to_method_polyglot, types::MadeMethod};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

pub async fn convert(source: &str, extension: &str, apply: bool) -> (String, MadeMethod) {
    let file = format!("case.{extension}");
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (&file, source),
    ]);
    let gateway = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let path = ws.root().join(file);
    let result = convert_to_method_polyglot(
        gateway.addr(),
        &ws.root(),
        &path,
        Some("C"),
        "same",
        apply,
        false,
    )
    .await
    .unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    if apply {
        assert_eq!(disk, result.rewritten[0].1);
    } else {
        assert_eq!(disk, source);
    }
    (result.rewritten[0].1.clone(), result)
}

pub fn run(program: &str, extension: &str, source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(format!("case.{extension}"));
    std::fs::write(&file, source).unwrap();
    let output = std::process::Command::new(program)
        .arg(&file)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{program}: {}\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

pub async fn refuses(source: &str, extension: &str) {
    let file = format!("case.{extension}");
    let ws = Workspace::new(&[(&file, source), ("caller.txt", "untouched")]);
    let path = ws.root().join(&file);
    for force in [false, true] {
        let result = convert_to_method_polyglot(
            "127.0.0.1:1".parse().unwrap(),
            &ws.root(),
            &path,
            Some("C"),
            "same",
            true,
            force,
        )
        .await;
        let error = result.unwrap_err().to_string();
        assert!(error.contains("unsupported"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), source);
        assert_eq!(
            std::fs::read_to_string(ws.root().join("caller.txt")).unwrap(),
            "untouched"
        );
    }
}

pub fn available(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

pub fn compiled(program: &str, extension: &str, source: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(format!("case.{extension}"));
    let binary = dir.path().join("case");
    std::fs::write(&file, source).unwrap();
    let mut command = std::process::Command::new(program);
    if extension == "cpp" {
        command.arg("-std=c++17");
    }
    let output = command.arg(&file).arg("-o").arg(&binary).output().unwrap();
    assert!(
        output.status.success(),
        "{program}: {}\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(binary).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
