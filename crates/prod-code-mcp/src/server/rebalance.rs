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
use std::path::Path;

/// Evaluates cluster rebalancing for the current workspace and pre-warms the target node
/// if a more efficient node is recommended. Rejects filesystem roots and unbounded non-git directories.
pub(crate) async fn handle_cluster_rebalance_tick(remote: &mut SocketAddr, workspace_root: &Path) {
    rebalance_tick_with(
        remote,
        workspace_root,
        |current, ws_name, engine, os| async move {
            crate::cluster::evaluate_cluster_rebalance(current, &ws_name, engine, os).await
        },
    )
    .await
}

/// [`handle_cluster_rebalance_tick`] with the cluster's verdict given: `evaluate(current node,
/// workspace name, engine, required OS)`.
async fn rebalance_tick_with<E, F>(remote: &mut SocketAddr, workspace_root: &Path, evaluate: E)
where
    E: FnOnce(SocketAddr, String, Option<&'static str>, Option<&'static str>) -> F,
    F: std::future::Future<Output = Option<(SocketAddr, String)>>,
{
    let canonical_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    if crate::sync::is_filesystem_root(&canonical_root) {
        return;
    }
    let has_git = crate::sync::git::git_head(&canonical_root).is_ok();
    let (_, engine) = crate::sync::engine_project(workspace_root, workspace_root);
    if !has_git && engine.is_none() {
        return;
    }

    let identity = crate::sync::workspace_identity(workspace_root);
    let ws_name = identity.base.as_ref().unwrap_or(&identity.name).clone();
    let os = if engine == Some("go") {
        crate::sync::macos_only_cgo(workspace_root).map(|_| "macos")
    } else {
        None
    };
    // Keep the active MCP session pinned to its current node while that node is reachable,
    // compatible, and free of hard memory/disk pressure (#1082): migrating mid-session on
    // soft CPU load (such as the load caused by our own `code_exec` build) strands built
    // artifacts in `target/` on the previous node.
    if crate::cluster::selection::node_fits(*remote, engine, os).await
        && let Ok(status) = crate::cluster::node_status(*remote).await
        && status.host.pressure().is_none()
    {
        return;
    }
    if let Some((new_addr, reason)) = evaluate(*remote, ws_name.clone(), engine, os).await {
        tracing::info!(
            old = %remote,
            new = %new_addr,
            %reason,
            "cluster rebalance: migrating active workspace to more efficient node"
        );
        *remote = new_addr;
        crate::cluster::remember_placement(&ws_name, new_addr);
        // Pre-warm the workspace on the new node in the background
        let root_clone = workspace_root.to_path_buf();
        let identity_clone = identity.clone();
        tokio::spawn(async move {
            if let Ok(stream) = prod_code_protocol::transport::connect(new_addr).await {
                let mut framed = tokio_util::codec::Framed::new(
                    stream,
                    prod_code_protocol::ProdCodeCodec::new(),
                );
                let _ = crate::sync::push_workspace_sync(
                    &mut framed,
                    &root_clone,
                    &identity_clone,
                    None,
                )
                .await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_closed_during_search_is_retryable_failover_error() {
        let err = anyhow::anyhow!("gateway closed the connection during the search");
        assert!(crate::server::failover::is_retryable_connection_error(
            "code_search",
            &err
        ));
    }

    #[tokio::test]
    async fn rebalance_tick_keeps_session_pinned_when_current_node_has_no_hard_pressure() {
        use futures_util::{SinkExt, StreamExt};
        use prod_code_protocol::{HostResources, ProdCodeCodec, StatusResponse, WireMessage};
        use tokio::net::TcpListener;
        use tokio_util::codec::Framed;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            // Serve node_fits + node_status queries with elevated CPU load (3800 / 4000 millis)
            // but default HostResources so `host.pressure().is_none()`.
            for _ in 0..2 {
                if let Ok((stream, _)) = listener.accept().await {
                    let mut framed = Framed::new(stream, ProdCodeCodec::new());
                    let _ = framed.next().await;
                    let _ = framed
                        .send(WireMessage::StatusResponse(StatusResponse {
                            server_pid: 1,
                            uptime_seconds: 120,
                            active_sessions: 1,
                            loaded_workspaces: 1,
                            detected_engines: vec!["rust (rust-analyzer)".to_string()],
                            memory_rss_bytes: None,
                            total_queries: 10,
                            active_queries: 0,
                            load_average_millis: Some(3800),
                            cpu_count: Some(4),
                            platform: Some("linux x86_64".to_string()),
                            running_commands: Vec::new(),
                            host: HostResources::default(),
                            version: Some("0.3.30".to_string()),
                            git_commit: None,
                        }))
                        .await;
                }
            }
        });

        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"sticky-test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let mut current_remote = addr;
        handle_cluster_rebalance_tick(&mut current_remote, temp.path()).await;
        assert_eq!(current_remote, addr);

        server.abort();
    }

    type Asked =
        std::sync::Arc<std::sync::Mutex<Vec<(SocketAddr, String, Option<String>, Option<String>)>>>;

    /// Runs the tick on `root` with `verdict` as the cluster's answer; returns the remote
    /// after the tick and every question the cluster was asked.
    async fn tick(
        root: &Path,
        remote: SocketAddr,
        verdict: Option<(SocketAddr, String)>,
    ) -> (
        SocketAddr,
        Vec<(SocketAddr, String, Option<String>, Option<String>)>,
    ) {
        let asked: Asked = Default::default();
        let log = asked.clone();
        let mut remote = remote;
        rebalance_tick_with(&mut remote, root, move |node, ws, engine, os| {
            log.lock()
                .unwrap()
                .push((node, ws, engine.map(String::from), os.map(String::from)));
            std::future::ready(verdict)
        })
        .await;
        let asked = asked.lock().unwrap().clone();
        (remote, asked)
    }

    fn closed_port() -> SocketAddr {
        // Port 1 is never listening: the node does not answer, so it cannot stay pinned.
        "127.0.0.1:1".parse().unwrap()
    }

    #[tokio::test]
    async fn rebalance_tick_ignores_roots_it_must_not_scan() {
        let new_node: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let verdict = Some((new_node, "never asked".to_string()));
        // A filesystem root.
        let (remote, asked) = tick(Path::new("/"), closed_port(), verdict.clone()).await;
        assert_eq!((remote, asked.len()), (closed_port(), 0));
        // A directory with neither a git history nor an engine manifest.
        let empty = tempfile::tempdir().unwrap();
        let (remote, asked) = tick(empty.path(), closed_port(), verdict).await;
        assert_eq!((remote, asked.len()), (closed_port(), 0));
    }

    #[tokio::test]
    async fn rebalance_tick_asks_the_cluster_when_the_node_cannot_stay() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"ask-test\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let identity = crate::sync::workspace_identity(temp.path());
        let ws = identity.base.clone().unwrap_or(identity.name.clone());

        // The cluster has nothing better: the session stays where it is.
        let (remote, asked) = tick(temp.path(), closed_port(), None).await;
        assert_eq!(remote, closed_port());
        assert_eq!(
            asked,
            [(closed_port(), ws.clone(), Some("rust".to_string()), None)]
        );

        // The cluster names a better node: the session moves and the node is pre-warmed.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let better = listener.local_addr().unwrap();
        let (remote, asked) =
            tick(temp.path(), closed_port(), Some((better, "roomier".into()))).await;
        assert_eq!(remote, better);
        assert_eq!(asked.len(), 1);
        let warmed =
            tokio::time::timeout(std::time::Duration::from_secs(5), listener.accept()).await;
        assert!(
            warmed.is_ok(),
            "the new node is connected to for the pre-warm sync"
        );
    }

    #[tokio::test]
    async fn rebalance_tick_requires_macos_for_a_go_module_that_only_builds_there() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("go.mod"), "module app\n\ngo 1.22\n").unwrap();
        let plain = "package main\n\nfunc main() {}\n";
        std::fs::write(temp.path().join("main.go"), plain).unwrap();
        let (_, asked) = tick(temp.path(), closed_port(), None).await;
        assert_eq!(
            (asked[0].2.as_deref(), asked[0].3.as_deref()),
            (Some("go"), None)
        );

        let cgo = "package c\n\n/*\n#include <libproc.h>\n*/\nimport \"C\"\n";
        std::fs::write(temp.path().join("proc.go"), cgo).unwrap();
        let (_, asked) = tick(temp.path(), closed_port(), None).await;
        assert_eq!(
            (asked[0].2.as_deref(), asked[0].3.as_deref()),
            (Some("go"), Some("macos"))
        );
    }
}
