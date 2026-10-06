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
use prod_code_protocol::{
    ClientCapabilities, HandshakeRequest, HandshakeResponse, PROTOCOL_VERSION, PathTranslator,
    ProdCodeCodec, StatusResponse, WireMessage,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

// We replicate the server loop setup for the test runner to bind to an ephemeral port (127.0.0.1:0)
async fn start_test_gateway() -> (SocketAddr, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    let temp_dir = tempfile::tempdir().unwrap();
    let storage_root = temp_dir.path().to_path_buf();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let state = Arc::new(ServerState {
        start_time: Instant::now(),
        server_pid: std::process::id(),
        next_session_id: std::sync::atomic::AtomicU64::new(1),
        active_sessions: std::sync::atomic::AtomicUsize::new(0),
        storage_root,
    });

    let handle = tokio::spawn(async move {
        while let Ok((socket, client_addr)) = listener.accept().await {
            let state_clone = Arc::clone(&state);
            tokio::spawn(async move {
                let _ = handle_test_client(socket, client_addr, state_clone).await;
            });
        }
    });

    (addr, handle, temp_dir)
}

struct ServerState {
    start_time: Instant,
    server_pid: u32,
    next_session_id: std::sync::atomic::AtomicU64,
    active_sessions: std::sync::atomic::AtomicUsize,
    storage_root: PathBuf,
}

async fn handle_test_client(
    socket: TcpStream,
    _addr: SocketAddr,
    state: Arc<ServerState>,
) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());

    while let Some(msg_res) = framed.next().await {
        let msg = msg_res?;
        match msg {
            WireMessage::StatusRequest => {
                let status = StatusResponse {
                    server_pid: state.server_pid,
                    uptime_seconds: state.start_time.elapsed().as_secs(),
                    active_sessions: state
                        .active_sessions
                        .load(std::sync::atomic::Ordering::Relaxed),
                    loaded_workspaces: 0,
                    detected_engines: vec!["rust (ra_ap_ide)".to_string()],
                    memory_rss_bytes: Some(1024 * 1024 * 50),
                    total_queries: 0,
                    active_queries: 0,
                    load_average_millis: None,
                    cpu_count: None,
                    platform: None,
                    running_commands: Vec::new(),
                    host: Default::default(),
                    version: None,
                    git_commit: None,
                };
                framed.send(WireMessage::StatusResponse(status)).await?;
            }
            WireMessage::HandshakeRequest(req) => {
                let session_id = state
                    .next_session_id
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                state
                    .active_sessions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let client_root_path = PathBuf::from(&req.client_workspace_root);
                let folder_name = client_root_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("default");
                let server_workspace = state.storage_root.join(folder_name);
                let server_workspace_str = server_workspace.to_string_lossy().to_string();

                let translator =
                    PathTranslator::new(&req.client_workspace_root, &server_workspace_str);

                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: state.server_pid,
                        session_id,
                        server_workspace_root: server_workspace_str,
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: None,
                        index_gated: false,
                        capabilities: None,
                    }))
                    .await?;

                // Session loop
                while let Some(sub_msg) = framed.next().await {
                    match sub_msg? {
                        WireMessage::LspPayload(client_json) => {
                            let server_json = translator.translate_lsp_to_server(&client_json);
                            if server_json.contains("initialize") {
                                let resp = serde_json::json!({
                                    "jsonrpc": "2.0",
                                    "id": 1,
                                    "result": {
                                        "capabilities": { "hoverProvider": true }
                                    }
                                });
                                let client_resp =
                                    translator.translate_lsp_to_client(&resp.to_string());
                                framed.send(WireMessage::LspPayload(client_resp)).await?;
                            } else {
                                let client_resp = translator.translate_lsp_to_client(&server_json);
                                framed.send(WireMessage::LspPayload(client_resp)).await?;
                            }
                        }
                        WireMessage::Disconnect { .. } => break,
                        _ => {}
                    }
                }

                state
                    .active_sessions
                    .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(());
            }
            _ => {}
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_full_phase1_e2e_flow() {
    let (server_addr, _handle, _temp_dir) = start_test_gateway().await;

    // 1. Test Status probe
    {
        let stream = TcpStream::connect(server_addr).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed.send(WireMessage::StatusRequest).await.unwrap();

        let resp = framed.next().await.unwrap().unwrap();
        match resp {
            WireMessage::StatusResponse(status) => {
                assert_eq!(status.active_sessions, 0);
                assert!(
                    status
                        .detected_engines
                        .contains(&"rust (ra_ap_ide)".to_string())
                );
            }
            other => panic!("Unexpected status response: {:?}", other),
        }
    }

    // 2. Test Handshake & LSP Roundtrip with Path Translation
    {
        let stream = TcpStream::connect(server_addr).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());

        let client_root = "/Users/dev/Documents/workspace/my-cool-project";
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                capabilities: None,
                client_name: "test-client".to_string(),
                client_pid: 9999,
                auth_token: None,
                client_workspace_root: client_root.to_string(),
                preferred_engine: None,
                base_workspace_name: None,
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
                redirect_count: 0,
            }))
            .await
            .unwrap();

        let resp = framed.next().await.unwrap().unwrap();
        let session_id = match resp {
            WireMessage::HandshakeResponse(resp) => {
                assert_eq!(resp.session_id, 1);
                assert!(resp.server_workspace_root.ends_with("my-cool-project"));
                assert_eq!(resp.detected_engine, "rust");
                resp.session_id
            }
            other => panic!("Unexpected handshake response: {:?}", other),
        };
        assert_eq!(session_id, 1);

        // Send LSP initialize request
        let lsp_req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "rootUri": format!("file://{client_root}")
            }
        });
        framed
            .send(WireMessage::LspPayload(lsp_req.to_string()))
            .await
            .unwrap();

        let lsp_resp_msg = framed.next().await.unwrap().unwrap();
        match lsp_resp_msg {
            WireMessage::LspPayload(json) => {
                let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
                assert_eq!(parsed["id"], 1);
                assert!(
                    parsed["result"]["capabilities"]["hoverProvider"]
                        .as_bool()
                        .unwrap()
                );
            }
            other => panic!("Unexpected LSP response: {:?}", other),
        }

        // Send clean disconnect
        framed
            .send(WireMessage::Disconnect {
                reason: "test completed".to_string(),
            })
            .await
            .unwrap();
    }

    // 3. Verify session was cleaned up
    {
        let stream = TcpStream::connect(server_addr).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed.send(WireMessage::StatusRequest).await.unwrap();

        let resp = framed.next().await.unwrap().unwrap();
        match resp {
            WireMessage::StatusResponse(status) => {
                assert_eq!(status.active_sessions, 0);
            }
            other => panic!("Unexpected status response: {:?}", other),
        }
    }
}

#[tokio::test]
async fn unix_socket_local_transport_and_negotiated_capabilities_e2e() {
    let temp_dir = tempfile::tempdir().unwrap();
    let socket_path = temp_dir.path().join("prod-code-test.sock");
    let storage = temp_dir.path().join("workspaces");
    std::fs::create_dir_all(&storage).unwrap();

    let cli = prod_code_gateway::ServerCli {
        bind: "127.0.0.1:0".parse().unwrap(),
        socket_path: Some(socket_path.clone()),
        storage,
        idle_evict_secs: 1800,
        engine_reserve_mib: 0,
        max_concurrent_engine_loads: 1,
        prune_worktree_secs: 3600,
        prune_worktree_days: Some(7),
        prune_workspace_secs: 86400,
        prune_workspace_days: None,
        prune_below_free_percent: 0,
        engines: vec![],
        shadow_dir: None,
        peers: String::new(),
        advertise: None,
        build_cache_ram: false,
        build_cache_dir: None,
        prometheus_listen: None,
        prometheus_push_url: None,
        prometheus_push_interval_secs: 15,
        prometheus_job: "prod-code".to_string(),
        prometheus_instance: None,
    };

    let server_task = tokio::spawn(async move {
        let _ = prod_code_gateway::run(cli).await;
    });

    // Wait until unix socket is created and ready
    let mut connected = false;
    for _ in 0..50 {
        if socket_path.exists()
            && let Ok(stream) = prod_code_protocol::transport::connect_unix(&socket_path).await
        {
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            if framed.send(WireMessage::Ping).await.is_ok()
                && let Some(Ok(WireMessage::Pong)) = framed.next().await
            {
                connected = true;
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        connected,
        "server failed to listen on unix domain socket within timeout"
    );

    // Connect via Unix socket and verify negotiated capabilities
    {
        let stream = prod_code_protocol::transport::connect_unix(&socket_path)
            .await
            .expect("connect via unix socket");
        let mut framed = Framed::new(stream, ProdCodeCodec::new());

        // Probe status over unix socket
        framed.send(WireMessage::StatusRequest).await.unwrap();
        let status = match framed.next().await.unwrap().unwrap() {
            WireMessage::StatusResponse(s) => s,
            other => panic!("expected StatusResponse, got {other:?}"),
        };
        assert!(status.server_pid > 0);

        // Perform handshake offering client capabilities
        let client_caps = ClientCapabilities {
            direct_edit: true,
            watch_files: true,
            indexing_status: true,
            shadow_runs: true,
            multi_root: true,
            sync_chunking: false,
            unix_socket_local: true,
            redirects: false,
        };

        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                capabilities: Some(client_caps.clone()),
                client_name: "unix-test-client".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: "/tmp/unix-test-ws".to_string(),
                preferred_engine: None,
                base_workspace_name: None,
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
                redirect_count: 0,
            }))
            .await
            .unwrap();

        // Check handshake response includes server capabilities
        let handshake_resp = match framed.next().await.unwrap().unwrap() {
            WireMessage::HandshakeResponse(resp) => resp,
            other => panic!("expected HandshakeResponse, got {other:?}"),
        };
        assert!(handshake_resp.capabilities.is_some());
        let server_caps = handshake_resp.capabilities.unwrap();
        assert!(server_caps.unix_socket_local);
        assert!(server_caps.direct_edit);

        // Send disconnect
        framed
            .send(WireMessage::Disconnect {
                reason: "unix test done".to_string(),
            })
            .await
            .unwrap();
    }

    server_task.abort();
}

#[tokio::test]
async fn transparent_gateway_redirection_e2e() {
    // 1. Start Node B (the warm target node)
    let (node_b_addr, node_b_handle, _dir_b) = start_test_gateway().await;

    // 2. Start Node A (redirector node that sends WireMessage::Redirect pointing to Node B)
    let listener_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let node_a_addr = listener_a.local_addr().unwrap();
    let target_b_str = node_b_addr.to_string();

    let node_a_handle = tokio::spawn(async move {
        while let Ok((socket, _)) = listener_a.accept().await {
            let target_addr = target_b_str.clone();
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(msg)) = framed.next().await {
                    if let WireMessage::HandshakeRequest(req) = msg
                        && req.redirect_count == 0
                    {
                        framed
                            .send(WireMessage::Redirect {
                                target_addr: target_addr.clone(),
                                reason: Some("workspace warm on Node B".to_string()),
                            })
                            .await
                            .unwrap();
                        return;
                    }
                }
            });
        }
    });

    // 3. Client connects to Node A, receives redirect, transparently reconnects to Node B
    let mut current_remote = node_a_addr;
    let mut redirect_count = 0;
    let client_root = "/Users/dev/Documents/workspace/my-redirect-project";

    let (mut framed, handshake_resp) = loop {
        let stream = TcpStream::connect(current_remote).await.unwrap();
        let mut framed = Framed::new(stream, ProdCodeCodec::new());

        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                capabilities: Some(ClientCapabilities {
                    redirects: true,
                    ..Default::default()
                }),
                client_name: "redirect-client".to_string(),
                client_pid: 12345,
                auth_token: None,
                client_workspace_root: client_root.to_string(),
                preferred_engine: None,
                base_workspace_name: None,
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
                redirect_count,
            }))
            .await
            .unwrap();

        match framed.next().await.unwrap().unwrap() {
            WireMessage::Redirect {
                target_addr,
                reason,
            } => {
                redirect_count += 1;
                assert!(redirect_count <= 2, "too many redirects");
                assert_eq!(reason.as_deref(), Some("workspace warm on Node B"));
                current_remote = target_addr.parse().expect("valid target address");
                continue;
            }
            WireMessage::HandshakeResponse(resp) => {
                break (framed, resp);
            }
            other => panic!("unexpected wire message: {other:?}"),
        }
    };

    assert_eq!(redirect_count, 1);
    assert_eq!(current_remote, node_b_addr);
    assert_eq!(handshake_resp.detected_engine, "rust");

    framed
        .send(WireMessage::Disconnect {
            reason: "redirect e2e test finished".to_string(),
        })
        .await
        .unwrap();

    node_a_handle.abort();
    node_b_handle.abort();
}
