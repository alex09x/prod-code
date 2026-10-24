/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::*;
use prod_code_testkit::Workspace;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

struct Node {
    addr: SocketAddr,
    calls: Arc<Mutex<Vec<ExecRequest>>>,
    placement: Arc<Mutex<SocketAddr>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Node {
    async fn start(text: &str, code: Option<i32>, error: Option<&str>, timed_out: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(Mutex::new(Vec::new()));
        let placement = Arc::new(Mutex::new(addr));
        let selected = placement.clone();
        let recorded = calls.clone();
        let text = text.to_owned();
        let error = error.map(str::to_owned);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    break;
                };
                let recorded = recorded.clone();
                let text = text.clone();
                let error = error.clone();
                let selected = selected.clone();
                tokio::spawn(async move {
                    let mut framed = Framed::new(socket, ProdCodeCodec::new());
                    while let Some(Ok(message)) = framed.next().await {
                        let response = match message {
                            WireMessage::PlaceRequest(_) | WireMessage::PlacePreviewRequest(_) => {
                                WireMessage::PlaceResponse(PlaceResponse {
                                    node: Some(selected.lock().unwrap().to_string()),
                                    reason: "test placement".into(),
                                })
                            }
                            WireMessage::StatusRequest => {
                                WireMessage::StatusResponse(StatusResponse {
                                    server_pid: 1,
                                    uptime_seconds: 1,
                                    active_sessions: 0,
                                    loaded_workspaces: 0,
                                    detected_engines: vec!["rust".into()],
                                    memory_rss_bytes: None,
                                    total_queries: 0,
                                    active_queries: 0,
                                    load_average_millis: None,
                                    cpu_count: None,
                                    platform: None,
                                    running_commands: vec![],
                                    host: Default::default(),
                                    version: None,
                                    git_commit: None,
                                })
                            }
                            WireMessage::SyncProbeRequest(req) => {
                                WireMessage::SyncProbeResponse(SyncProbeResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    seeded: false,
                                    files_deleted: 0,
                                    missing: vec![],
                                })
                            }
                            WireMessage::SyncRequest(req) => {
                                WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: 0,
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    workspace_was_fresh: false,
                                    stale_paths: vec![],
                                })
                            }
                            WireMessage::ExecRequest(req) => {
                                recorded.lock().unwrap().push(req.clone());
                                if req.command == ["generate-sample"] {
                                    assert!(req.pull_changes);
                                    if framed
                                        .send(WireMessage::ExecChanges(ExecChanges {
                                            files: vec![FileDelta {
                                                relative_path: "generated.txt".into(),
                                                content: Some(b"sample result\n".to_vec()),
                                                is_executable: false,
                                            }],
                                        }))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                if !text.is_empty()
                                    && framed
                                        .send(WireMessage::ExecChunk(ExecChunk {
                                            stderr: true,
                                            data: Some(text.as_bytes().to_vec()),
                                        }))
                                        .await
                                        .is_err()
                                {
                                    break;
                                }
                                WireMessage::ExecExit(ExecExit {
                                    exit_code: code,
                                    duration_ms: 1,
                                    server_workspace_root: req.client_workspace_root,
                                    timed_out,
                                    error: error.clone(),
                                    usage: Some(ExecUsage {
                                        cpu_user_ms: 1000,
                                        cpu_sys_ms: 500,
                                        max_rss_kb: 2048,
                                    }),
                                    platform: Some("sample-platform".into()),
                                })
                            }
                            _ => break,
                        };
                        if framed.send(response).await.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        Self {
            addr,
            calls,
            placement,
            task,
        }
    }
}
fn workspace() -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname=\"sample\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        ),
        ("src/lib.rs", "pub fn sample() {}\n"),
    ])
}

#[tokio::test]
async fn exec_copies_back_generated_file_and_names_it() {
    let ws = workspace();
    let node = Node::start("", Some(0), None, false).await;
    let output = cli(
        &ws,
        &node.addr.to_string(),
        &["exec", "--", "generate-sample"],
        false,
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    assert_eq!(ws.read("generated.txt"), "sample result\n");
    assert!(err(&output).contains(
        "[prod-code exec] 1 file(s) changed by the command written back: generated.txt\n"
    ));
}
async fn cli(ws: &Workspace, remotes: &str, args: &[&str], nested: bool) -> std::process::Output {
    tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"))
        .args(["-r", remotes])
        .args(args)
        .env("HOME", ws.root())
        .current_dir(if nested { ws.path("src") } else { ws.root() })
        .output()
        .await
        .unwrap()
}
fn out(output: &std::process::Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}
fn err(output: &std::process::Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap()
}

#[tokio::test]
async fn verification_reports_all_nodes_rejecting_msrv() {
    let ws = workspace();
    let text = "error: rustc 1.80.0 is not supported by the following packages:\n  sample requires rustc 1.97.1\n";
    for mode in ["--json", "--events", "text"] {
        let first = Node::start(text, Some(101), None, false).await;
        let second = Node::start(text, Some(101), None, false).await;
        *second.placement.lock().unwrap() = first.addr;
        let addresses = format!("{},{}", first.addr, second.addr);
        let args = if mode == "text" {
            vec!["check"]
        } else {
            vec!["check", mode]
        };
        let output = cli(&ws, &addresses, &args, false).await;
        assert_eq!(output.status.code(), Some(1), "{}", err(&output));
        assert_eq!(first.calls.lock().unwrap().len(), 1);
        assert_eq!(second.calls.lock().unwrap().len(), 1);
        if mode == "--json" {
            let report: serde_json::Value = serde_json::from_str(&out(&output)).unwrap();
            assert_eq!(report["exit_code"], 101);
            assert_eq!(report["tail"], text);
        } else if mode == "--events" {
            let lines: Vec<serde_json::Value> = out(&output)
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let report = lines.last().unwrap();
            assert_eq!(report["event"], "report");
            assert_eq!(report["report"]["exit_code"], 101);
        } else {
            assert!(
                err(&output).contains(&format!(
                    "workspace requires a newer Rust toolchain than available on {}",
                    second.addr
                )),
                "{}",
                err(&output)
            );
            assert!(
                out(&output).contains("sample requires rustc 1.97.1"),
                "{}",
                out(&output)
            );
        }
    }
}

#[tokio::test]
async fn verification_falls_back_to_compatible_node() {
    let ws = workspace();
    let first = Node::start(
        "error: rustc 1.80.0 is not supported by the following packages:\n",
        Some(101),
        None,
        false,
    )
    .await;
    let second = Node::start("", Some(0), None, false).await;
    *second.placement.lock().unwrap() = first.addr;
    let output = cli(
        &ws,
        &format!("{},{}", first.addr, second.addr),
        &["check", "--json", "--env", "SAMPLE=a=b"],
        false,
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let report: serde_json::Value = serde_json::from_str(&out(&output)).unwrap();
    assert_eq!(report["exit_code"], 0);
    for node in [&first, &second] {
        let requests = node.calls.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].env.contains(&("SAMPLE".into(), "a=b".into())));
    }
}

#[tokio::test]
async fn exec_errors_and_termination_are_reported() {
    let ws = workspace();
    for (code, error, timed_out, expected) in [
        (None, None, false, "killed"),
        (None, None, true, "timed out"),
        (
            Some(1),
            Some("cannot start sample"),
            false,
            "Error: remote exec failed: cannot start sample",
        ),
    ] {
        let node = Node::start("", code, error, timed_out).await;
        let output = cli(
            &ws,
            &node.addr.to_string(),
            &["exec", "--", "sample"],
            false,
        )
        .await;
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(out(&output), "");
        assert!(err(&output).contains(expected), "{}", err(&output));
        if error.is_none() {
            assert!(err(&output).contains("cpu 1.0s user 0.5s sys, peak 2 MB"));
            assert!(err(&output).contains("(sample-platform)"));
        }
        assert_eq!(node.calls.lock().unwrap()[0].command, ["sample"]);
    }
}

#[tokio::test]
async fn grep_empty_results_scopes_root_and_nested_directory() {
    let ws = workspace();
    for nested in [false, true] {
        let node = Node::start("", Some(1), None, false).await;
        let output = cli(&ws, &node.addr.to_string(), &["grep", "absent"], nested).await;
        assert_eq!(output.status.code(), Some(0), "{}", err(&output));
        assert_eq!(out(&output), "No matches found for `absent`.\n");
        let requests = node.calls.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].command.contains(&"absent".to_string()));
        assert!(
            requests[0]
                .command
                .contains(&if nested { "src".into() } else { ".".into() })
        );
    }
}

#[tokio::test]
async fn malformed_environment_and_shadow_input_refuse_before_exec() {
    let ws = workspace();
    ws.write("bad.json", "{");
    let node = Node::start("", Some(0), None, false).await;
    for (args, expected) in [
        (
            vec!["exec", "--env", "BROKEN", "--", "sample"],
            "--env takes KEY=VALUE, got `BROKEN`",
        ),
        (
            vec!["check", "--env", "BROKEN"],
            "--env takes KEY=VALUE, got `BROKEN`",
        ),
        (
            vec!["check", "--fix", "--path", "absent"],
            "/absent does not exist",
        ),
        (
            vec!["shadow-run", "bad.json", "--", "sample"],
            "bad.json is not JSON",
        ),
    ] {
        let output = cli(&ws, &node.addr.to_string(), &args, false).await;
        assert_eq!(output.status.code(), Some(1));
        assert!(err(&output).contains(expected), "{}", err(&output));
        assert_eq!(out(&output), "");
    }
    assert_eq!(node.calls.lock().unwrap().len(), 0);
}
