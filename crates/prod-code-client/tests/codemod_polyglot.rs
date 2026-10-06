/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::protocol::{McpContentItem, McpToolCallResult};
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::json;
use std::net::SocketAddr;
use std::process::Output;
use std::time::Instant;

fn tool_text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| {
            let McpContentItem::Text { text } = item;
            text.as_str()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn cli(ws: &Workspace, remote: SocketAddr, args: &[&str]) -> Output {
    let mut cmd = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    cmd.current_dir(ws.root())
        .arg(format!("--remote={remote}"))
        .args(args);
    cmd.output().await.expect("execute prod-code CLI")
}

#[tokio::test]
async fn polyglot_codemod_go_api_migration() {
    let ws = Workspace::new(&[
        (
            "pkg/server/handler.go",
            "package server\n\nimport \"errors\"\n\nfunc HandleRequest() error {\n\terr := fetch()\n\tif err != nil {\n\t\treturn errors.Wrap(err, \"fetch failed\")\n\t}\n\treturn nil\n}\n",
        ),
    ]);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;

    // 1. Dry run preview via CLI
    let dry_run = cli(
        &ws,
        gw.addr(),
        &[
            "codemod",
            "errors.Wrap($err, $msg) ==>> wrapError($err, $msg)",
            "--path",
            "pkg/server/handler.go",
        ],
    )
    .await;
    assert!(dry_run.status.success());
    let dry_text = stdout(&dry_run);
    assert!(dry_text.contains("2 changed line(s) in 1 file(s)"));
    assert!(dry_text.contains("errors.Wrap(err, \"fetch failed\")"));
    assert!(dry_text.contains("wrapError(err, \"fetch failed\")"));
    assert!(dry_text.contains("nothing was written; pass `apply: true` to make these edits"));

    // File on disk must remain unchanged after dry-run
    let content_before = std::fs::read_to_string(ws.root().join("pkg/server/handler.go")).unwrap();
    assert!(content_before.contains("errors.Wrap"));

    // 2. Apply the edit
    let applied = cli(
        &ws,
        gw.addr(),
        &[
            "codemod",
            "errors.Wrap($err, $msg) ==>> wrapError($err, $msg)",
            "--path",
            "pkg/server/handler.go",
            "--apply",
        ],
    )
    .await;
    assert!(applied.status.success());
    let applied_text = stdout(&applied);
    assert!(applied_text.contains("[applied to 1 file(s): pkg/server/handler.go]"));

    // File on disk must now be updated
    let content_after = std::fs::read_to_string(ws.root().join("pkg/server/handler.go")).unwrap();
    assert!(content_after.contains("return wrapError(err, \"fetch failed\")"));
}

#[tokio::test]
async fn polyglot_codemod_typescript_api_upgrade() {
    let ws = Workspace::new(&[
        (
            "src/auth.ts",
            "export function login(user: User): void {\n    console.log(\"user login: \" + user.name);\n}\n",
        ),
        (
            "src/service.ts",
            "export function logout(id: string): void {\n    console.log(\"user logout: \" + id);\n}\n",
        ),
    ]);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;

    // MCP tool call with apply: true
    let result = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_codemod",
        json!({
            "rule": "console.log($msg) ==>> logger.info($msg)",
            "apply": true,
        }),
    )
    .await
    .expect("codemod tool execution");

    let text = tool_text(&result);
    assert!(text.contains("4 changed line(s) in 2 file(s)"));
    assert!(text.contains("[applied to 2 file(s):"));

    let auth_after = std::fs::read_to_string(ws.root().join("src/auth.ts")).unwrap();
    assert!(auth_after.contains("logger.info(\"user login: \" + user.name);"));

    let service_after = std::fs::read_to_string(ws.root().join("src/service.ts")).unwrap();
    assert!(service_after.contains("logger.info(\"user logout: \" + id);"));
}

#[tokio::test]
async fn polyglot_codemod_python_library_modernization() {
    let ws = Workspace::new(&[
        (
            "app/config.py",
            "import os\n\ndef load():\n    cfg = os.path.join(base_dir, \"settings.yaml\")\n    return cfg\n",
        ),
    ]);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;

    let result = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_codemod",
        json!({
            "rule": "os.path.join($a, $b) ==>> Path($a) / $b",
            "path": "app/config.py",
            "apply": true,
        }),
    )
    .await
    .expect("codemod tool execution");

    let text = tool_text(&result);
    assert!(text.contains("2 changed line(s) in 1 file(s)"));

    let py_after = std::fs::read_to_string(ws.root().join("app/config.py")).unwrap();
    assert!(py_after.contains("cfg = Path(base_dir) / \"settings.yaml\""));
}

#[tokio::test]
async fn polyglot_codemod_cpp_and_swift_refactors() {
    let ws = Workspace::new(&[
        (
            "src/factory.cpp",
            "#include <memory>\n\nvoid create() {\n    auto w = std::make_shared<Widget>(1, 2);\n}\n",
        ),
        (
            "Sources/App/Logger.swift",
            "import Foundation\n\nfunc logTrace() {\n    print(\"event fired\")\n}\n",
        ),
    ]);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;

    // C++ refactor
    let cpp_res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_codemod",
        json!({
            "rule": "std::make_shared<$T>($args) ==>> std::allocate_shared<$T>(alloc, $args)",
            "path": "src/factory.cpp",
            "apply": true,
        }),
    )
    .await
    .expect("cpp codemod");

    assert!(tool_text(&cpp_res).contains("2 changed line(s) in 1 file(s)"));
    let cpp_after = std::fs::read_to_string(ws.root().join("src/factory.cpp")).unwrap();
    assert!(cpp_after.contains("auto w = std::allocate_shared<Widget>(alloc, 1, 2);"));

    // Swift refactor
    let swift_res = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_codemod",
        json!({
            "rule": "print($x) ==>> os_log($x)",
            "path": "Sources/App/Logger.swift",
            "apply": true,
        }),
    )
    .await
    .expect("swift codemod");

    assert!(tool_text(&swift_res).contains("2 changed line(s) in 1 file(s)"));
    let swift_after = std::fs::read_to_string(ws.root().join("Sources/App/Logger.swift")).unwrap();
    assert!(swift_after.contains("os_log(\"event fired\")"));
}

#[tokio::test]
async fn polyglot_codemod_subsecond_hundred_files_benchmark() {
    // Generate 100 source files across 6 languages
    let mut files = Vec::new();
    for i in 0..100 {
        let path = match i % 6 {
            0 => format!("src/module_{i}.rs"),
            1 => format!("pkg/service_{i}.go"),
            2 => format!("frontend/component_{i}.ts"),
            3 => format!("services/worker_{i}.py"),
            4 => format!("native/engine_{i}.cpp"),
            _ => format!("Sources/Model_{i}.swift"),
        };
        let content = format!("// File {i}\nfn run_{i}() {{\n    oldApiCall(data_{i}, {i});\n}}\n");
        files.push((path, content));
    }

    let file_slices: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();

    let ws = Workspace::new(&file_slices);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;

    let start = Instant::now();
    let result = prod_code_mcp::tools::execute_tool(
        gw.addr(),
        &ws.root(),
        "code_codemod",
        json!({
            "rule": "oldApiCall($a, $b) ==>> newApiCall($b, $a)",
            "apply": true,
        }),
    )
    .await
    .expect("100-file benchmark codemod");

    let duration = start.elapsed();
    let text = tool_text(&result);

    assert!(text.contains("200 changed line(s) in 100 file(s)"));
    assert!(text.contains("[applied to 100 file(s):"));

    // Verify sub-second migration requirement (< 800ms)
    assert!(
        duration.as_millis() < 800,
        "Codemod across 100 files took {:?}, expected < 800ms",
        duration
    );

    // Verify rewritten content
    let sample = std::fs::read_to_string(ws.root().join("pkg/service_1.go")).unwrap();
    assert!(sample.contains("newApiCall(1, data_1);"));
}
