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

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    ClusterResponse, HostResources, PeerInfo, ProdCodeCodec, StatusResponse, WireMessage,
};
use tokio_util::codec::Framed;

/// A node that answers a status request with the engines it serves and nothing else.
pub(crate) async fn node_serving(engines: &'static [&'static str]) -> SocketAddr {
    node_on(engines, Some("linux x86_64")).await
}

/// A node that answers a status request with the engines it serves and the platform it
/// runs, `None` for a gateway too old to report one.
pub(crate) async fn node_on(
    engines: &'static [&'static str],
    platform: Option<&'static str>,
) -> SocketAddr {
    node_with(engines, platform, 100, HostResources::default()).await
}

/// A node that answers a status request with the engines it serves, the platform it runs,
/// its load average in thousandths over 4 CPUs, and what its host has left.
pub(crate) async fn node_with(
    engines: &'static [&'static str],
    platform: Option<&'static str>,
    load_average_millis: u32,
    host: HostResources,
) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let host = host.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    if matches!(message, WireMessage::StatusRequest) {
                        let status = StatusResponse {
                            server_pid: 1,
                            uptime_seconds: 1,
                            active_sessions: 0,
                            loaded_workspaces: 0,
                            detected_engines: engines.iter().map(|e| e.to_string()).collect(),
                            memory_rss_bytes: None,
                            total_queries: 0,
                            active_queries: 0,
                            load_average_millis: Some(load_average_millis),
                            cpu_count: Some(4),
                            platform: platform.map(String::from),
                            running_commands: Vec::new(),
                            host: host.clone(),
                            version: None,
                            git_commit: None,
                        };
                        let _ = framed.send(WireMessage::StatusResponse(status)).await;
                    } else {
                        return;
                    }
                }
            });
        }
    });
    addr
}

pub(crate) async fn seed_node_with_peers(peers: Vec<SocketAddr>) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let peers = peers.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    match message {
                        WireMessage::ClusterRequest => {
                            let resp = ClusterResponse {
                                this_node: addr.to_string(),
                                nodes: peers
                                    .iter()
                                    .map(|p| PeerInfo {
                                        addr: p.to_string(),
                                        status: StatusResponse {
                                            server_pid: 1,
                                            uptime_seconds: 1,
                                            active_sessions: 0,
                                            loaded_workspaces: 0,
                                            detected_engines: vec!["rust".to_string()],
                                            memory_rss_bytes: None,
                                            total_queries: 0,
                                            active_queries: 0,
                                            load_average_millis: Some(100),
                                            cpu_count: Some(4),
                                            platform: Some("linux x86_64".to_string()),
                                            running_commands: Vec::new(),
                                            host: HostResources::default(),
                                            version: None,
                                            git_commit: None,
                                        },
                                        last_seen_secs: 0,
                                        workspaces: vec![],
                                        alive: true,
                                    })
                                    .collect(),
                            };
                            let _ = framed.send(WireMessage::ClusterResponse(resp)).await;
                        }
                        WireMessage::StatusRequest => {
                            let status = StatusResponse {
                                server_pid: 1,
                                uptime_seconds: 1,
                                active_sessions: 0,
                                loaded_workspaces: 0,
                                detected_engines: vec!["rust".to_string()],
                                memory_rss_bytes: None,
                                total_queries: 0,
                                active_queries: 0,
                                load_average_millis: Some(100),
                                cpu_count: Some(4),
                                platform: Some("linux x86_64".to_string()),
                                running_commands: Vec::new(),
                                host: HostResources::default(),
                                version: None,
                                git_commit: None,
                            };
                            let _ = framed.send(WireMessage::StatusResponse(status)).await;
                        }
                        _ => return,
                    }
                }
            });
        }
    });
    addr
}
