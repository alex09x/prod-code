/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Workspace node pins through the CLI against an isolated fake mesh (#1793): every gateway is a
//! local listener that records what it was asked, so a test can prove that no work reached a node
//! outside the pin, that an old gateway is refused before a command runs, and that the reports
//! name the node that actually ran the work.

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::*;
use prod_code_testkit::Workspace;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::net::TcpListener;
use tokio_util::codec::Framed;

#[path = "pin_policy/gates.rs"]
mod gates;

const CONFIRMED_NODE: &str = "pinned-node.example:9400";

/// How a fake gateway behaves.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Confirms node pins and runs commands.
    Current,
    /// Older than node pins: cannot decode the request and closes the connection.
    Old,
    /// Closes the connection on the pre-flight sync: the command never started.
    DropsSync,
    /// Closes the connection after taking the command: its result is unknown.
    DropsExec,
}

struct Node {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Node {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Node {
    /// A gateway of `kind` with load `load` (thousandths over 4 CPUs) whose placement answer
    /// names `place`.
    async fn start(kind: Kind, load: u32, place: Option<SocketAddr>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                log.lock().unwrap().push("connect".to_string());
                tokio::spawn(serve(socket, kind, load, place, log.clone()));
            }
        });
        Self { addr, seen, task }
    }

    fn seen(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    fn received(&self, kind: &str) -> bool {
        self.seen().iter().any(|seen| seen == kind)
    }
}

fn name(message: &WireMessage) -> String {
    serde_json::to_value(message).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn serve(
    socket: tokio::net::TcpStream,
    kind: Kind,
    load: u32,
    place: Option<SocketAddr>,
    seen: Arc<Mutex<Vec<String>>>,
) {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(Ok(message)) = framed.next().await {
        seen.lock().unwrap().push(name(&message));
        let answer = match message {
            WireMessage::NodePinRequest(_) if kind == Kind::Old => return,
            WireMessage::NodePinRequest(_) => WireMessage::NodePinResponse(NodePinResponse {
                accepted: true,
                node: Some(CONFIRMED_NODE.into()),
                reason: None,
            }),
            WireMessage::StatusRequest => WireMessage::StatusResponse(StatusResponse {
                detected_engines: vec!["rust".into()],
                load_average_millis: Some(load),
                cpu_count: Some(4),
                platform: Some("linux x86_64".into()),
                ..Default::default()
            }),
            WireMessage::PlaceRequest(_) | WireMessage::PlacePreviewRequest(_) => {
                WireMessage::PlaceResponse(PlaceResponse {
                    node: place.map(|p| p.to_string()),
                    reason: "test placement".into(),
                    notified_sessions: 0,
                })
            }
            WireMessage::SyncProbeRequest(_) if kind == Kind::DropsSync => return,
            WireMessage::SyncProbeRequest(req) => {
                WireMessage::SyncProbeResponse(SyncProbeResponse {
                    server_workspace_root: req.client_workspace_root,
                    seeded: false,
                    files_deleted: 0,
                    missing: vec![],
                })
            }
            WireMessage::SyncRequest(req) => WireMessage::SyncResponse(SyncResponse {
                server_workspace_root: req.client_workspace_root,
                files_updated: 0,
                files_deleted: 0,
                bytes_transferred: 0,
                duration_ms: 0,
                workspace_was_fresh: false,
                stale_paths: vec![],
            }),
            WireMessage::ExecRequest(_) if kind == Kind::DropsExec => return,
            WireMessage::ExecRequest(req) => WireMessage::ExecExit(ExecExit {
                exit_code: Some(0),
                duration_ms: 1,
                server_workspace_root: req.client_workspace_root,
                timed_out: false,
                error: None,
                usage: None,
                platform: Some("linux x86_64".into()),
            }),
            _ => return,
        };
        if framed.send(answer).await.is_err() {
            return;
        }
    }
}

fn workspace(extra: &[(&str, &str)]) -> Workspace {
    let mut files = vec![
        (
            "Cargo.toml",
            "[package]\nname=\"pinned\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        ),
        ("src/lib.rs", "pub fn pinned() {}\n"),
    ];
    files.extend_from_slice(extra);
    Workspace::new(&files)
}

/// Runs the CLI in `ws` with only the environment given: no inherited remote, cluster or pin.
async fn cli(ws: &Workspace, args: &[&str], envs: &[(&str, String)]) -> std::process::Output {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_prod-code"));
    command
        .args(args)
        .env("HOME", ws.root())
        .env_remove("PROD_CODE_PIN_NODE")
        .env_remove("PROD_CODE_REMOTE")
        .env_remove("PROD_CODE_CLUSTER")
        .env_remove("PROD_CODE_SEEDS")
        .current_dir(ws.root())
        .kill_on_drop(true);
    for (key, value) in envs {
        command.env(key, value);
    }
    tokio::time::timeout(Duration::from_secs(90), command.output())
        .await
        .expect("the CLI must finish")
        .unwrap()
}

fn err(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn out(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[tokio::test]
async fn issue_1793_strict_pin_runs_exec_only_on_the_pinned_node_and_names_it() {
    let ws = workspace(&[]);
    let pinned = Node::start(Kind::Current, 0, None).await;
    let seed = Node::start(Kind::Current, 0, None).await;
    // The environment's seed alone is not a pin; the explicit pin wins over it.
    let output = cli(
        &ws,
        &["--pin-node", &pinned.addr.to_string(), "exec", "--", "true"],
        &[("PROD_CODE_REMOTE", seed.addr.to_string())],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let seen = pinned.seen();
    let pin_at = seen.iter().position(|s| s == "NodePinRequest").unwrap();
    let sync_at = seen.iter().position(|s| s == "SyncProbeRequest").unwrap();
    assert!(pin_at < sync_at, "the pin is confirmed first: {seen:?}");
    assert!(pinned.received("ExecRequest"));
    assert!(
        seed.seen().is_empty(),
        "no work reached the seed: {:?}",
        seed.seen()
    );
    let stderr = err(&output);
    assert!(
        stderr.contains(&format!(
            "on node {} (gateway confirms {CONFIRMED_NODE})",
            pinned.addr
        )),
        "{stderr}"
    );
}

#[tokio::test]
async fn issue_1793_an_old_gateway_is_refused_before_a_command_runs() {
    let ws = workspace(&[]);
    let old = Node::start(Kind::Old, 0, None).await;
    let output = cli(
        &ws,
        &["--pin-node", &old.addr.to_string(), "exec", "--", "true"],
        &[],
    )
    .await;
    assert_ne!(output.status.code(), Some(0));
    let stderr = err(&output);
    assert!(stderr.contains("older than node pins"), "{stderr}");
    assert!(old.received("NodePinRequest"));
    assert!(!old.received("SyncProbeRequest"), "{:?}", old.seen());
    assert!(!old.received("ExecRequest"), "{:?}", old.seen());

    // Without a strict pin the same old gateway keeps working, and is never asked to pin.
    let listed = Node::start(Kind::Old, 0, None).await;
    let output = cli(
        &ws,
        &["--remote", &listed.addr.to_string(), "exec", "--", "true"],
        &[],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    assert!(listed.received("ExecRequest"));
    assert!(!listed.received("NodePinRequest"));
}

#[tokio::test]
async fn issue_1793_workspace_config_pin_holds_verification_and_reports_the_node() {
    let pinned = Node::start(Kind::Current, 0, None).await;
    let seed = Node::start(Kind::Current, 0, None).await;
    let config = format!("[cluster]\npin = \"{}\"\n", pinned.addr);
    let ws = workspace(&[("prod-code.toml", config.as_str())]);
    let output = cli(
        &ws,
        &["check", "--json"],
        &[("PROD_CODE_REMOTE", seed.addr.to_string())],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let report: serde_json::Value = serde_json::from_str(&out(&output)).unwrap();
    assert_eq!(report["execution"]["node"], pinned.addr.to_string());
    assert_eq!(report["execution"]["gateway_node"], CONFIRMED_NODE);
    assert_eq!(report["execution"]["relocations"], serde_json::json!([]));
    assert!(pinned.received("ExecRequest"));
    assert!(seed.seen().is_empty(), "{:?}", seed.seen());

    // `--pin-node auto` turns the workspace pin off; the typed remote is then the set.
    let output = cli(
        &ws,
        &[
            "--pin-node",
            "auto",
            "--remote",
            &seed.addr.to_string(),
            "check",
            "--json",
        ],
        &[],
    )
    .await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let report: serde_json::Value = serde_json::from_str(&out(&output)).unwrap();
    assert_eq!(report["execution"]["node"], seed.addr.to_string());
    assert!(!seed.received("NodePinRequest"));
}

#[tokio::test]
async fn issue_1793_cli_retry_stays_in_the_listed_set_and_reports_the_move() {
    let ws = workspace(&[]);
    let unlisted = Node::start(Kind::Current, 0, None).await;
    // The quieter listed node is picked first, and drops the pre-flight sync.
    let first = Node::start(Kind::DropsSync, 0, Some(unlisted.addr)).await;
    let second = Node::start(Kind::Current, 3600, Some(unlisted.addr)).await;
    let listed = format!("{},{}", first.addr, second.addr);
    let output = cli(&ws, &["--remote", &listed, "check", "--events"], &[]).await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let events: Vec<serde_json::Value> = out(&output)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let moved = events
        .iter()
        .find(|e| e["event"] == "relocated")
        .expect("the move is an event");
    assert_eq!(moved["from"], first.addr.to_string());
    assert_eq!(moved["to"], second.addr.to_string());
    let report = events.last().unwrap();
    assert_eq!(report["event"], "report");
    assert_eq!(report["execution"]["node"], second.addr.to_string());
    assert_eq!(report["execution"]["requested"], first.addr.to_string());
    assert!(second.received("ExecRequest"));
    assert!(!first.received("ExecRequest"));
    assert!(
        unlisted.seen().is_empty(),
        "no placement or work reached the unlisted node: {:?}",
        unlisted.seen()
    );
}

#[tokio::test]
async fn issue_1793_a_possibly_started_verification_is_not_retried_elsewhere() {
    let ws = workspace(&[]);
    let first = Node::start(Kind::DropsExec, 0, None).await;
    let second = Node::start(Kind::Current, 3600, None).await;
    let listed = format!("{},{}", first.addr, second.addr);
    let output = cli(&ws, &["--remote", &listed, "check", "--json"], &[]).await;
    assert_ne!(output.status.code(), Some(0));
    let stderr = err(&output);
    assert!(stderr.contains("result is unknown"), "{stderr}");
    assert!(
        stderr.contains(&format!("on node {}", first.addr)),
        "{stderr}"
    );
    assert!(first.received("ExecRequest"));
    assert!(!second.received("ExecRequest"), "{:?}", second.seen());
    assert!(!second.received("SyncProbeRequest"), "{:?}", second.seen());
}

#[tokio::test]
async fn issue_1793_an_unreachable_strict_pin_fails_closed() {
    let ws = workspace(&[]);
    let seed = Node::start(Kind::Current, 0, None).await;
    // A port nothing listens on.
    let dead = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    };
    let output = cli(
        &ws,
        &["--pin-node", &dead.to_string(), "check"],
        &[("PROD_CODE_REMOTE", seed.addr.to_string())],
    )
    .await;
    assert_ne!(output.status.code(), Some(0));
    let stderr = err(&output);
    assert!(
        stderr.contains(&format!("pinned to {dead} (--pin-node)")),
        "{stderr}"
    );
    assert!(seed.seen().is_empty(), "{:?}", seed.seen());

    // A pin outside a typed --remote set is a conflict, refused before anything runs.
    let output = cli(
        &ws,
        &[
            "--pin-node",
            &dead.to_string(),
            "--remote",
            &seed.addr.to_string(),
            "exec",
            "--",
            "true",
        ],
        &[],
    )
    .await;
    assert_ne!(output.status.code(), Some(0));
    assert!(
        err(&output).contains("does not include"),
        "{}",
        err(&output)
    );
    assert!(seed.seen().is_empty(), "{:?}", seed.seen());
}
