/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{PlaceResponse, ProdCodeCodec, StatusResponse, WireMessage};
use tokio_util::codec::Framed;

use crate::cluster::{checkout_node_in, pick_node_with, route_in};

#[path = "route_placement_regressions.rs"]
mod placement;

#[path = "route_go_roots.rs"]
mod go_roots;

#[path = "route_local_deps.rs"]
mod local_deps;

#[path = "route_also_in.rs"]
mod also_in;

struct Node {
    addr: SocketAddr,
    state: Arc<Mutex<StatusResponse>>,
    requests: Arc<Mutex<Vec<Option<String>>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Node {
    async fn new(platform: &str, engines: &[&str], recommend_self: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(StatusResponse {
            platform: Some(platform.into()),
            detected_engines: engines.iter().map(|e| (*e).into()).collect(),
            cpu_count: Some(4),
            load_average_millis: Some(0),
            ..Default::default()
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let status = state.clone();
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut wire = Framed::new(socket, ProdCodeCodec::new());
                let reply = match wire.next().await {
                    Some(Ok(WireMessage::StatusRequest)) => {
                        Some(WireMessage::StatusResponse(status.lock().unwrap().clone()))
                    }
                    Some(Ok(WireMessage::PlaceRequest(request))) => {
                        captured.lock().unwrap().push(request.os);
                        Some(WireMessage::PlaceResponse(PlaceResponse {
                            node: recommend_self.then(|| addr.to_string()),
                            reason: "simulated placement".into(),
                        }))
                    }
                    _ => None,
                };
                if let Some(reply) = reply {
                    let _ = wire.send(reply).await;
                }
            }
        });
        Self {
            addr,
            state,
            requests,
            task,
        }
    }

    async fn stop(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"fixture\"\n").unwrap();
    std::fs::create_dir_all(root.join("tools/apple/sub")).unwrap();
    std::fs::write(
        root.join("tools/apple/go.mod"),
        "module example.org/apple\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tools/apple/sub/main.go"),
        "package main\n/*\n#include <CoreFoundation/CoreFoundation.h>\n*/\nimport \"C\"\n",
    )
    .unwrap();
    temp
}

async fn nested_cgo(default_go: bool) {
    let temp = fixture();
    let linux = Node::new("linux x86_64", &["rust", "go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let rust = Node::new("linux x86_64", &["rust"], false).await;
    let default = if default_go { linux.addr } else { rust.addr };
    for path in ["tools/apple", "tools/apple/sub/main.go"] {
        let chosen = route_in(
            &[linux.addr, mac.addr, rust.addr],
            "fixture",
            default,
            temp.path(),
            path,
            None,
        )
        .await
        .unwrap();
        assert_eq!(chosen, mac.addr, "default_go={default_go}, path={path}");
    }
    assert!(
        linux
            .requests
            .lock()
            .unwrap()
            .iter()
            .all(|os| os.as_deref() == Some("macos"))
    );
    // Rust root queries must not scan the unrelated nested Go utility.
    assert_eq!(
        route_in(
            &[linux.addr, mac.addr],
            "fixture",
            linux.addr,
            temp.path(),
            ".",
            None
        )
        .await
        .unwrap(),
        linux.addr
    );
    assert_eq!(
        checkout_node_in(
            &[linux.addr, mac.addr],
            "different",
            linux.addr,
            temp.path(),
            None
        )
        .await
        .unwrap(),
        linux.addr
    );
    linux.stop().await;
    mac.stop().await;
    rust.stop().await;
}

#[tokio::test]
async fn issue_1330_nested_cgo_default() {
    nested_cgo(true).await;
}

#[tokio::test]
async fn issue_1330_nested_cgo_fallback() {
    nested_cgo(false).await;
}

#[tokio::test]
async fn issue_1330_ordinary_go_and_selected_nested_module() {
    let temp = fixture();
    let root = temp.path();
    std::fs::create_dir_all(root.join("tools/plain/sub")).unwrap();
    std::fs::write(
        root.join("tools/plain/go.mod"),
        "module example.org/plain\n",
    )
    .unwrap();
    std::fs::write(root.join("tools/plain/sub/main.go"), "package main\n").unwrap();
    let linux = Node::new("linux x86_64", &["rust", "go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let nodes = [linux.addr, mac.addr];
    for go_root in [false, true] {
        if go_root {
            std::fs::remove_file(root.join("Cargo.toml")).unwrap();
            std::fs::write(root.join("go.mod"), "module example.org/root\n").unwrap();
        }
        for path in ["tools/plain", "tools/plain/sub/main.go"] {
            assert_eq!(
                route_in(&nodes, "fixture", linux.addr, root, path, None)
                    .await
                    .unwrap(),
                linux.addr,
                "{path}, go_root={go_root}"
            );
        }
        let absolute = root.join("tools/apple/sub/main.go");
        assert_eq!(
            route_in(
                &nodes,
                "fixture",
                linux.addr,
                root,
                &absolute.to_string_lossy(),
                None
            )
            .await
            .unwrap(),
            mac.addr
        );
    }
    linux.stop().await;
    mac.stop().await;
}

#[tokio::test]
async fn issue_1330_portable_parent_excludes_independent_child_module() {
    let temp = fixture();
    let root = temp.path();
    std::fs::create_dir_all(root.join("tools/plain/apple")).unwrap();
    for (path, source) in [
        ("tools/plain/go.mod", "module example.org/plain\n"),
        ("tools/plain/main.go", "package main\n"),
        ("tools/plain/apple/go.mod", "module example.org/apple\n"),
        (
            "tools/plain/apple/main.go",
            "package main\n/*\n#include <CoreFoundation/CoreFoundation.h>\n*/\nimport \"C\"\n",
        ),
    ] {
        std::fs::write(root.join(path), source).unwrap();
    }
    let linux = Node::new("linux x86_64", &["rust", "go"], false).await;
    let mac = Node::new("macos aarch64", &["go"], false).await;
    let rust = Node::new("linux x86_64", &["rust"], false).await;
    // Exercise both the compatible-default shortcut and fallback, including the
    // single available Go node: an unrelated child must not require a Mac.
    for nodes in [vec![linux.addr], vec![linux.addr, mac.addr, rust.addr]] {
        for default in [linux.addr, rust.addr] {
            for path in ["tools/plain", "tools/plain/main.go"] {
                assert_eq!(
                    route_in(&nodes, "portable-parent", default, root, path, None)
                        .await
                        .unwrap(),
                    linux.addr,
                    "{path}, default={default}"
                );
            }
        }
    }
    // Selecting the child directly must still discover its own OS constraint.
    for default in [linux.addr, rust.addr] {
        for path in ["tools/plain/apple", "tools/plain/apple/main.go"] {
            assert_eq!(
                route_in(
                    &[linux.addr, mac.addr, rust.addr],
                    "apple-child",
                    default,
                    root,
                    path,
                    None
                )
                .await
                .unwrap(),
                mac.addr,
                "{path}, default={default}"
            );
        }
    }
    linux.stop().await;
    mac.stop().await;
    rust.stop().await;
}

async fn os_probe_error(multiple: bool) {
    // Bound but not listening: deterministic refusal, with the port reserved throughout.
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    let nodes = if multiple {
        vec![addr, addr]
    } else {
        vec![addr]
    };
    let err = pick_node_with(&nodes, "apple", Some("go"), Some("macos"), None)
        .await
        .context("routing nested Go project")
        .unwrap_err();
    let chain = format!("{err:#}");
    assert!(chain.contains("routing nested Go project"), "{chain}");
    assert!(chain.contains("runs macOS"), "{chain}");
    assert!(chain.contains("Connection refused"), "{chain}");
    assert!(
        err.root_cause().downcast_ref::<std::io::Error>().is_some(),
        "{chain}"
    );
}

#[tokio::test]
async fn issue_1330_os_probe_error_single() {
    os_probe_error(false).await;
}

#[tokio::test]
async fn issue_1330_os_probe_error_multiple() {
    os_probe_error(true).await;
}
