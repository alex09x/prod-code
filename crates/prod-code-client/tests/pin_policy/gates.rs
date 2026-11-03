/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The pin gate on every CLI connection that syncs or can run work (#1793), and the blank pin
//! that must not shadow a workspace's own.

use super::*;

/// Frames that carry the workspace or start work; none may reach an unconfirmed gateway.
const WORK: &[&str] = &[
    "SyncProbeRequest",
    "SyncRequest",
    "HandshakeRequest",
    "LspPayload",
    "ExecRequest",
    "SearchRequest",
    "PullFileRequest",
];

#[tokio::test]
async fn issue_1793_cli_sync_query_and_editor_send_nothing_to_an_old_gateway() {
    let ws = workspace(&[]);
    for args in [
        &["sync"][..],
        &["hover", "src/lib.rs", "1", "8"][..],
        &["lsp"][..],
    ] {
        let old = Node::start(Kind::Old, 0, None).await;
        let pin = old.addr.to_string();
        let mut full = vec!["--pin-node", pin.as_str()];
        full.extend_from_slice(args);
        let output = cli(&ws, &full, &[]).await;
        let seen = old.seen();
        assert!(
            seen.iter().any(|s| s == "NodePinRequest"),
            "{args:?} reached the gate: {seen:?}\n{}",
            err(&output)
        );
        for frame in WORK {
            assert!(
                !seen.iter().any(|s| s == frame),
                "{args:?} sent {frame} to a gateway that never confirmed the pin: {seen:?}"
            );
        }
        if args[0] != "lsp" {
            assert_ne!(output.status.code(), Some(0), "{args:?}");
            assert!(
                err(&output).contains("older than node pins"),
                "{}",
                err(&output)
            );
        }
    }
}

#[tokio::test]
async fn issue_1793_cli_sync_confirms_on_a_current_gateway_first() {
    let ws = workspace(&[]);
    let pinned = Node::start(Kind::Current, 0, None).await;
    let pin = pinned.addr.to_string();
    let output = cli(&ws, &["--pin-node", &pin, "sync"], &[]).await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let seen: Vec<String> = pinned
        .seen()
        .into_iter()
        .filter(|s| s != "connect" && s != "StatusRequest")
        .collect();
    assert_eq!(
        seen.first().map(String::as_str),
        Some("NodePinRequest"),
        "{seen:?}"
    );
    assert!(seen.iter().any(|s| s == "SyncProbeRequest"), "{seen:?}");
}

#[tokio::test]
async fn issue_1793_a_blank_pin_variable_keeps_the_workspace_pin() {
    let pinned = Node::start(Kind::Current, 0, None).await;
    let seed = Node::start(Kind::Current, 0, None).await;
    let config = format!("[cluster]\npin = \"{}\"\n", pinned.addr);
    let ws = workspace(&[("prod-code.toml", config.as_str())]);
    for blank in ["", "   "] {
        let output = cli(
            &ws,
            &["check", "--json"],
            &[
                ("PROD_CODE_PIN_NODE", blank.to_string()),
                ("PROD_CODE_REMOTE", seed.addr.to_string()),
            ],
        )
        .await;
        assert_eq!(output.status.code(), Some(0), "{blank:?}: {}", err(&output));
        let report: serde_json::Value = serde_json::from_str(&out(&output)).unwrap();
        assert_eq!(report["execution"]["node"], pinned.addr.to_string());
        assert_eq!(report["execution"]["gateway_node"], CONFIRMED_NODE);
        assert_eq!(report["executor"]["confirmed_node"], CONFIRMED_NODE);
        assert!(seed.seen().is_empty(), "{blank:?}: {:?}", seed.seen());
    }
}

fn committed_pinned_repo(pin: SocketAddr) -> Workspace {
    let config = format!("[cluster]\npin = \"{pin}\"\n");
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname=\"pinned_target\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
        ),
        ("src/lib.rs", "pub fn pinned() -> usize {\n    1\n}\n"),
        ("prod-code.toml", config.as_str()),
    ])
}

#[tokio::test]
async fn issue_1793_cli_benchmarks_send_one_pin_frame_and_no_work_to_an_old_gateway() {
    let ws = workspace(&[]);
    for args in [
        &["bench", "--concurrency", "2", "--duration-secs", "1"][..],
        &["divergent-bench", "--mode", "shared"][..],
    ] {
        let old = Node::start(Kind::Old, 0, None).await;
        let pin = old.addr.to_string();
        let mut full = vec!["--pin-node", pin.as_str()];
        full.extend_from_slice(args);
        let output = cli(&ws, &full, &[]).await;
        let seen = old.seen();
        let pin_frames = seen
            .iter()
            .filter(|s| s.as_str() == "NodePinRequest")
            .count();
        assert_eq!(
            pin_frames,
            1,
            "{args:?} must stop after one NodePinRequest on an old gateway: {seen:?}\n{}",
            err(&output)
        );
        for frame in WORK {
            assert!(
                !seen.iter().any(|s| s == frame),
                "{args:?} sent {frame} to a gateway that never confirmed the pin: {seen:?}"
            );
        }
        assert_ne!(output.status.code(), Some(0), "{args:?}");
        assert!(
            err(&output).contains("older than node pins"),
            "{}",
            err(&output)
        );
    }
}

#[tokio::test]
async fn issue_1793_cross_root_benchmarks_honor_target_pin_and_skip_cwd_gateway() {
    let cwd_ws = workspace(&[]);

    // 1. Target B pinned to an old gateway: cwd's node A sees no benchmark traffic, and old B sees
    //    exactly one NodePinRequest and zero Sync/Handshake/LSP frames.
    for kind in ["bench", "divergent-bench"] {
        let node_a = Node::start(Kind::Current, 0, None).await;
        let old_b = Node::start(Kind::Old, 0, None).await;
        let target_b = committed_pinned_repo(old_b.addr);
        let remote_a = node_a.addr.to_string();
        let target_str = target_b.root().to_str().unwrap().to_string();
        let args: Vec<&str> = if kind == "bench" {
            vec![
                "bench",
                "--workspaces",
                &target_str,
                "--concurrency",
                "2",
                "--duration-secs",
                "1",
            ]
        } else {
            vec![
                "divergent-bench",
                "--base-repo",
                &target_str,
                "--mode",
                "shared",
            ]
        };
        let output = cli(&cwd_ws, &args, &[("PROD_CODE_REMOTE", remote_a)]).await;
        assert_ne!(output.status.code(), Some(0), "{kind}: {}", err(&output));
        assert!(
            err(&output).contains("older than node pins"),
            "{kind}: {}",
            err(&output)
        );
        let seen_a = node_a.seen();
        assert!(
            !seen_a.iter().any(|s| s == "NodePinRequest"),
            "{kind} sent NodePinRequest to cwd node A: {seen_a:?}"
        );
        for frame in WORK {
            assert!(
                !seen_a.iter().any(|s| s == frame),
                "{kind} sent {frame} to cwd node A instead of pinned B: {seen_a:?}"
            );
        }
        let seen_b = old_b.seen();
        assert_eq!(
            seen_b
                .iter()
                .filter(|s| s.as_str() == "NodePinRequest")
                .count(),
            1,
            "{kind} on old B: {seen_b:?}"
        );
        for frame in WORK {
            assert!(
                !seen_b.iter().any(|s| s == frame),
                "{kind} sent {frame} to unconfirmed old B: {seen_b:?}"
            );
        }
    }

    // 2. Target B pinned to a current gateway (which confirms NodePinRequest and closes before any
    //    LSP session workload): cwd's node A still sees zero benchmark frames, and every connection
    //    to B starts with NodePinRequest before Sync/Handshake.
    for kind in ["bench", "divergent-bench"] {
        let node_a = Node::start(Kind::Current, 0, None).await;
        let current_b = Node::start(Kind::Current, 0, None).await;
        let target_b = committed_pinned_repo(current_b.addr);
        let remote_a = node_a.addr.to_string();
        let target_str = target_b.root().to_str().unwrap().to_string();
        let args: Vec<&str> = if kind == "bench" {
            vec![
                "bench",
                "--workspaces",
                &target_str,
                "--concurrency",
                "1",
                "--duration-secs",
                "1",
            ]
        } else {
            vec!["divergent-bench", "--base-repo", &target_str]
        };
        let _ = cli(&cwd_ws, &args, &[("PROD_CODE_REMOTE", remote_a)]).await;
        let seen_a = node_a.seen();
        assert!(
            !seen_a.iter().any(|s| s == "NodePinRequest"),
            "{kind} sent NodePinRequest to cwd node A: {seen_a:?}"
        );
        for frame in WORK {
            assert!(
                !seen_a.iter().any(|s| s == frame),
                "{kind} sent {frame} to cwd node A instead of pinned B: {seen_a:?}"
            );
        }
        let seen_b = current_b.seen();
        let connects: Vec<usize> = seen_b
            .iter()
            .enumerate()
            .filter_map(|(idx, s)| (s == "connect").then_some(idx))
            .collect();
        assert!(
            !connects.is_empty(),
            "{kind} must connect to target B's pinned gateway: {seen_b:?}"
        );
        if kind == "divergent-bench" {
            // Isolated mode exercises both `initial_sync` and `open_session` before stopping at
            // the fake gateway's unimplemented HandshakeRequest.
            assert_eq!(
                connects.len(),
                2,
                "divergent-bench connections on B: {seen_b:?}"
            );
        }
        for idx in connects {
            assert_eq!(
                seen_b.get(idx + 1).map(String::as_str),
                Some("NodePinRequest"),
                "{kind} connection at {idx} did not confirm NodePinRequest first: {seen_b:?}"
            );
        }
        assert!(
            !seen_b.iter().any(|s| s == "LspPayload"),
            "{kind} must stop before any LSP benchmark workload: {seen_b:?}"
        );
    }
}
