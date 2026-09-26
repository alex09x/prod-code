use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, HandshakeResponse, PROTOCOL_VERSION, PathTranslator, ProdCodeCodec,
    StatusResponse, WireMessage,
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
