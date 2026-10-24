/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{is_retryable_connection_error, rediscover_node_with};
use anyhow::anyhow;
use prod_code_protocol::StatusResponse;
use prod_code_protocol::discovery::{DiscoveredNode, LoadedWorkspace};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn addr(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

#[test]
fn retryable_errors_are_the_ones_where_nothing_ran() {
    let retryable = [
        "Failed to connect to remote gateway at 127.0.0.1:9400",
        "Connection refused (os error 111)",
        "No route to host",
        "Network is unreachable",
        "operation TIMED OUT",
        "request timeout",
        "Broken pipe",
        "Connection reset by peer",
        "connection closed before the reply",
        "the peer closed the connection",
        "Transport closed",
        "server closed connection",
        "capacity: this node has no memory for a new engine",
        "the gateway refused the session: capacity: busy",
        "this node has no memory for a new rust engine",
        "session refused for capacity",
        "capacity admission failed",
        "No space left on device",
        "node is short of disk",
        "pre-flight workspace sync failed",
        "unexpected sync probe response",
    ];
    for msg in retryable {
        let e = anyhow!("{msg}");
        assert!(
            is_retryable_connection_error("code_definition", &e),
            "{msg}"
        );
        // The exec tool is never replayed, whatever the message says.
        assert!(!is_retryable_connection_error("code_exec", &e), "{msg}");
    }
    let not_retryable = [
        "syntax error in file.rs",
        "",
        "lost the connection to the gateway during exec (connection reset); the command's result is unknown",
        "gateway closed the connection; the result is unknown",
        "connection refused, but the command's result is unknown",
    ];
    for msg in not_retryable {
        let e = anyhow!("{msg}");
        assert!(!is_retryable_connection_error("code_check", &e), "{msg}");
    }
    // The whole context chain is read, not only the outermost message.
    let chained = anyhow!("Connection refused (os error 111)").context("tool call failed");
    assert!(is_retryable_connection_error("code_symbols", &chained));
}

fn workspace(name: &str) -> LoadedWorkspace {
    LoadedWorkspace {
        name: name.into(),
        engine: "rust".into(),
        sessions: 1,
    }
}

fn node(
    port: u16,
    engines: &[&str],
    mem_avail_mb: u64,
    load: f64,
    loaded: &[&str],
) -> DiscoveredNode {
    DiscoveredNode {
        addr: addr(port),
        engines: engines.iter().map(|e| e.to_string()).collect(),
        rss_mb: 100,
        load_per_cpu: load,
        cpus: 8,
        mem_total_mb: 16000,
        mem_avail_mb,
        sessions: 0,
        workspaces: loaded.iter().map(|n| workspace(n)).collect(),
        nonce: None,
    }
}

/// A checkout named `app` holding `files`, so the workspace identity and engine are known.
fn checkout(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("app");
    std::fs::create_dir_all(&root).unwrap();
    for (name, text) in files {
        std::fs::write(root.join(name), text).unwrap();
    }
    (dir, root)
}

const OLD: u16 = 9400;
const CARGO: (&str, &str) = (
    "Cargo.toml",
    "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
);

async fn pick(
    root: &std::path::Path,
    known: Option<Vec<SocketAddr>>,
    nodes: Vec<DiscoveredNode>,
) -> (Option<SocketAddr>, Vec<SocketAddr>) {
    let seeds = Arc::new(Mutex::new(Vec::new()));
    let seen = seeds.clone();
    let picked = rediscover_node_with(
        addr(OLD),
        root,
        known,
        move |s| {
            *seen.lock().unwrap() = s;
            std::future::ready(nodes)
        },
        |_| std::future::ready(Err(anyhow!("no tcp in this test"))),
    )
    .await;
    let seeds = seeds.lock().unwrap().clone();
    (picked, seeds)
}

#[tokio::test]
async fn discovery_picks_the_roomiest_quiet_node_and_never_the_dead_one() {
    let (_dir, root) = checkout(&[]);
    // (nodes answering, expected pick, why)
    let cases: Vec<(Vec<DiscoveredNode>, Option<u16>, &str)> = vec![
        (
            vec![],
            None,
            "nobody answered and there is nothing to fall back on",
        ),
        (
            vec![node(OLD, &[], 4000, 0.1, &[])],
            Some(OLD),
            "the old node is the only one that answered",
        ),
        (
            vec![
                node(OLD, &[], 9000, 0.0, &[]),
                node(9411, &[], 2000, 0.0, &[]),
            ],
            Some(9411),
            "the old node is excluded when another answered",
        ),
        (
            vec![
                node(9411, &[], 4000, 0.1, &[]),
                node(9412, &[], 8000, 0.9, &[]),
            ],
            Some(9411),
            "load costs 10000 MB per CPU of load: 4000-1000 beats 8000-9000",
        ),
        (
            vec![
                node(9411, &[], 8000, 0.0, &[]),
                node(9412, &[], 600, 0.0, &["app"]),
            ],
            Some(9412),
            "a node with the workspace loaded and 512 MB or more wins over a roomier one",
        ),
        (
            vec![
                node(9411, &[], 8000, 0.0, &[]),
                node(9412, &[], 100, 0.0, &["app"]),
            ],
            Some(9411),
            "a warm node short of memory is not preferred",
        ),
        (
            vec![
                node(9411, &[], 8000, 0.0, &[]),
                node(9412, &[], 9000, 0.0, &["other"]),
            ],
            Some(9412),
            "another workspace's residency is not warmth: the roomiest wins",
        ),
        (
            vec![
                node(9411, &[], 100, 0.5, &[]),
                node(9412, &[], 300, 0.1, &[]),
            ],
            Some(9412),
            "every node is short of memory: the best score of all candidates",
        ),
    ];
    for (nodes, expected, why) in cases {
        let (picked, _) = pick(&root, None, nodes).await;
        assert_eq!(picked, expected.map(addr), "{why}");
    }
}

#[tokio::test]
async fn seeds_are_the_old_node_then_the_known_ones_without_repeats() {
    let (_dir, root) = checkout(&[]);
    let known = vec![addr(9411), addr(OLD), addr(9412), addr(9411)];
    let (_, seeds) = pick(&root, Some(known), vec![]).await;
    assert_eq!(seeds, [addr(OLD), addr(9411), addr(9412)]);
    let (_, seeds) = pick(&root, None, vec![]).await;
    assert_eq!(seeds, [addr(OLD)]);
}

#[tokio::test]
async fn discovery_only_picks_nodes_that_serve_the_checkouts_engine() {
    let (_rust_dir, rust) = checkout(&[CARGO]);
    let (_swift_dir, swift) = checkout(&[("Package.swift", "// swift-tools-version:5.9\n")]);
    let go_src = "package c\n\n/*\n#include <libproc.h>\n*/\nimport \"C\"\n";
    let (_go_dir, cgo) = checkout(&[("go.mod", "module app\n\ngo 1.22\n"), ("proc.go", go_src)]);
    let (_plain_dir, plain_go) = checkout(&[
        ("go.mod", "module app\n\ngo 1.22\n"),
        ("main.go", "package main\n"),
    ]);
    let peers = |engines: &[&str]| vec![node(9411, engines, 4000, 0.0, &[])];
    // (checkout, engines the peer lists, expected)
    let cases: Vec<(&PathBuf, &[&str], Option<u16>)> = vec![
        (&rust, &["rust"], Some(9411)),
        (&rust, &["rust (rust-analyzer)"], Some(9411)),
        (&rust, &["rustic"], None),
        (&rust, &["go", "swift"], None),
        (&swift, &["swift (sourcekit-lsp)"], Some(9411)),
        (&swift, &["rust"], None),
        (&plain_go, &["go"], Some(9411)),
        (&cgo, &["go"], None),
        (&cgo, &["go (gopls)", "swift (sourcekit-lsp)"], Some(9411)),
    ];
    for (root, engines, expected) in cases {
        let (picked, _) = pick(root, None, peers(engines)).await;
        assert_eq!(picked, expected.map(addr), "{root:?} on {engines:?}");
    }
}

fn status(platform: &str, engines: &[&str], load_millis: u32) -> StatusResponse {
    StatusResponse {
        detected_engines: engines.iter().map(|e| e.to_string()).collect(),
        platform: Some(platform.into()),
        load_average_millis: Some(load_millis),
        cpu_count: Some(1),
        ..StatusResponse::default()
    }
}

async fn fallback(
    root: &std::path::Path,
    known: Vec<SocketAddr>,
    table: Vec<(u16, StatusResponse)>,
) -> Option<SocketAddr> {
    let table: HashMap<SocketAddr, StatusResponse> =
        table.into_iter().map(|(p, s)| (addr(p), s)).collect();
    rediscover_node_with(
        addr(OLD),
        root,
        Some(known),
        |_| std::future::ready(vec![]),
        |a| std::future::ready(table.get(&a).cloned().ok_or_else(|| anyhow!("{a} is down"))),
    )
    .await
}

#[tokio::test]
async fn without_a_udp_answer_the_known_nodes_are_asked_over_tcp() {
    let (_dir, plain) = checkout(&[]);
    let (_rdir, rust) = checkout(&[CARGO]);
    let (_sdir, swift) = checkout(&[("Package.swift", "// swift-tools-version:5.9\n")]);
    let known = vec![addr(9411), addr(9412), addr(9413)];
    let linux = |engines: &[&str], load| status("linux x86_64", engines, load);
    let mac = |engines: &[&str], load| status("macos aarch64", engines, load);

    // The quietest answering peer; the old node is never asked, an unreachable one is skipped.
    let busy_and_quiet = vec![
        (OLD, linux(&[], 0)),
        (9411, linux(&[], 900)),
        (9412, linux(&[], 100)),
    ];
    assert_eq!(
        fallback(&plain, known.clone(), busy_and_quiet).await,
        Some(addr(9412))
    );
    assert_eq!(fallback(&plain, known.clone(), vec![]).await, None);
    assert_eq!(
        fallback(&plain, vec![], vec![(OLD, linux(&[], 0))]).await,
        None
    );

    // The engine filter.
    let by_engine = vec![
        (9411, linux(&["go"], 0)),
        (9412, linux(&["rust (rust-analyzer)"], 800)),
    ];
    assert_eq!(
        fallback(&rust, known.clone(), by_engine).await,
        Some(addr(9412))
    );

    // A Swift checkout needs a macOS node that serves swift.
    let by_os = vec![
        (9411, linux(&["swift (sourcekit-lsp)"], 0)),
        (9412, mac(&["rust"], 0)),
        (9413, mac(&["swift (sourcekit-lsp)"], 500)),
    ];
    assert_eq!(fallback(&swift, known, by_os).await, Some(addr(9413)));
}
