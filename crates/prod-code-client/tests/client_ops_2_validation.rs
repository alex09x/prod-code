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
use std::process::{Output, Stdio};
use tokio::io::AsyncWriteExt;

fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname='fixture'\nversion='0.1.0'\nedition='2021'\n",
        ),
        ("src/lib.rs", "pub fn original() {}\n"),
    ])
}

async fn run(
    ws: &Workspace,
    cache: &std::path::Path,
    gateway: &ScriptedGateway,
    args: &[&str],
    input: &str,
) -> Output {
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .arg("--remote")
        .arg(gateway.addr().to_string())
        .args(args)
        .env("HOME", cache)
        .env("XDG_CACHE_HOME", cache)
        .env_remove("PROD_CODE_CLUSTER")
        .current_dir(ws.root())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(input.as_bytes()).await.unwrap();
    drop(stdin);
    tokio::time::timeout(std::time::Duration::from_secs(5), child.wait_with_output())
        .await
        .expect("CLI deadline")
        .unwrap()
}

#[tokio::test]
async fn streams_from_stdin_and_files_report_exact_final_state() {
    let ws = workspace();
    let cache = tempfile::tempdir().unwrap();
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" {
            answers::no_diagnostics()
        } else {
            json!([])
        }
    })
    .await;
    let source = "pub fn sample() {\r\n    // 界\r\n}\r\n";
    ws.write("proposal.rs", source);
    for from_file in [false, true] {
        for json_output in [false, true] {
            let mut args = vec![
                "validate",
                "src/lib.rs",
                "--stream",
                "--session",
                "stream-fixture",
            ];
            if from_file {
                args.extend(["--from", "proposal.rs"]);
            }
            if json_output {
                args.push("--json");
            }
            let out = run(
                &ws,
                cache.path(),
                &gateway,
                &args,
                if from_file { "" } else { source },
            )
            .await;
            assert_eq!(
                out.status.code(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            if json_output {
                let report: Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(report["session_id"], "stream-fixture");
                assert_eq!(report["chunk_index"], if from_file { 3 } else { 4 });
                assert_eq!(report["accumulated_bytes"], source.len());
                assert_eq!(report["intercepted"], false);
                assert_eq!(report["final_report"]["errors"], 0);
                assert_eq!(
                    report["summary"],
                    "stream generation validated clean: 0 hallucinations intercepted"
                );
            } else {
                assert!(String::from_utf8_lossy(&out.stdout).lines().any(|line| line == format!("Stream chunk {} [stream-fixture]: stream generation validated clean: 0 hallucinations intercepted", if from_file { 3 } else { 4 })));
            }
            assert_eq!(ws.read("src/lib.rs"), "pub fn original() {}\n");
        }
    }
}

#[tokio::test]
async fn streamed_hallucinations_terminate_stdin_and_file_modes() {
    let ws = workspace();
    let cache = tempfile::tempdir().unwrap();
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" {
            answers::error_at(
                1,
                20,
                "unresolved-method",
                "no method named `absent` found for struct `Sample`",
            )
        } else {
            json!([])
        }
    })
    .await;
    let source = "pub fn sample() { value.absent(); }\n";
    ws.write("proposal.rs", source);
    for from_file in [false, true] {
        for json_output in [false, true] {
            let mut args = vec!["validate", "src/lib.rs", "--stream"];
            if from_file {
                args.extend(["--from", "proposal.rs"]);
            }
            if json_output {
                args.push("--json");
            }
            let out = run(
                &ws,
                cache.path(),
                &gateway,
                &args,
                if from_file { "" } else { source },
            )
            .await;
            assert_eq!(out.status.code(), Some(1));
            if json_output {
                let report: Value = serde_json::from_slice(&out.stdout).unwrap();
                assert_eq!(report["intercepted"], true);
                assert_eq!(report["chunk_index"], if from_file { 1 } else { 2 });
                assert_eq!(report["accumulated_bytes"], source.len());
                assert_eq!(report["interception"]["symbol_or_target"], "absent");
            } else {
                assert!(
                    String::from_utf8_lossy(&out.stdout).contains(&format!(
                        "[INTERCEPT at chunk {}]",
                        if from_file { 1 } else { 2 }
                    )),
                    "{}",
                    String::from_utf8_lossy(&out.stdout)
                );
                assert!(String::from_utf8_lossy(&out.stderr).contains(if from_file {
                    "INTERCEPTED on-the-fly"
                } else {
                    "1 chunk(s) streamed and validated in"
                }));
            }
        }
    }
}

#[tokio::test]
async fn persisted_chunk_state_is_replayed_and_closed_across_processes() {
    let ws = workspace();
    let cache = tempfile::tempdir().unwrap();
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" {
            answers::no_diagnostics()
        } else {
            json!([])
        }
    })
    .await;
    let first = "pub struct Sample;\n";
    let second = "impl Sample { pub fn value(&self) {} }\n";
    for (chunk, flag, index, bytes) in [
        (first, "--reset", 1, first.len()),
        (second, "--close", 2, first.len() + second.len()),
    ] {
        let out = run(
            &ws,
            cache.path(),
            &gateway,
            &[
                "validate",
                "src/lib.rs",
                "--chunk",
                chunk,
                "--session",
                "persist-fixture",
                flag,
                "--json",
            ],
            "",
        )
        .await;
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["session_id"], "persist-fixture");
        assert_eq!(report["chunk_index"], index);
        assert_eq!(report["accumulated_bytes"], bytes);
        assert_eq!(report["intercepted"], false);
    }
    let out = run(
        &ws,
        cache.path(),
        &gateway,
        &[
            "validate",
            "src/lib.rs",
            "--chunk",
            "",
            "--session",
            "persist-fixture",
        ],
        "",
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("CLI stream session is already closed; start a new session with --reset")
    );
}

#[tokio::test]
async fn multi_file_validation_refuses_malformed_pairs_and_missing_proposals() {
    let ws = workspace();
    let cache = tempfile::tempdir().unwrap();
    let gateway =
        ScriptedGateway::start(|_, _| panic!("invalid proposal must not reach analyzer")).await;
    for compile in [false, true] {
        for (pair, expected) in [
            ("broken", "--with takes FILE=NEW, got `broken`"),
            ("src/other.rs=missing.rs", "failed to read missing.rs"),
        ] {
            let mut args = vec![
                "validate",
                "src/lib.rs",
                "--from",
                "src/lib.rs",
                "--with",
                pair,
            ];
            if compile {
                args.push("--compile");
            }
            let out = run(&ws, cache.path(), &gateway, &args, "").await;
            assert_eq!(out.status.code(), Some(1));
            assert!(
                String::from_utf8_lossy(&out.stderr).contains(expected),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }
}
