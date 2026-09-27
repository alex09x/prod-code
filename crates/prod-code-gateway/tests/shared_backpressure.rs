#![cfg(unix)]

use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::workspace::{SharedWorkspace, WorkspaceManager};
use prod_code_gateway::{ACTIVE_SHARED_OUTPUT_WRITERS, ServerState, handle_client};
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::Duration;
use tokio_util::codec::Framed;

const WAIT: Duration = Duration::from_secs(6);

struct OwnedHandler {
    task: Option<JoinHandle<anyhow::Result<()>>>,
}

impl OwnedHandler {
    fn spawn(listener: TcpListener, state: Arc<ServerState>) -> Self {
        Self {
            task: Some(tokio::spawn(async move {
                let (socket, peer) = listener.accept().await.expect("accept shared client");
                handle_client(socket, peer, state).await
            })),
        }
    }

    async fn finishes_within(mut self, budget: Duration) -> Result<anyhow::Result<()>, Self> {
        let task = self.task.as_mut().expect("owned handler");
        match tokio::time::timeout(budget, &mut *task).await {
            Ok(joined) => {
                self.task.take();
                Ok(joined.expect("shared handler does not panic"))
            }
            Err(_) => Err(self),
        }
    }

    async fn abort_and_join(mut self) {
        let task = self.task.as_mut().expect("owned handler");
        task.abort();
        let joined = tokio::time::timeout(WAIT, &mut *task)
            .await
            .expect("aborted handler joins");
        self.task.take();
        assert!(
            joined
                .expect_err("aborted handler must not complete")
                .is_cancelled(),
            "handler cancellation is reported"
        );
    }
}

impl Drop for OwnedHandler {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

async fn shared_server() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    Arc<ServerState>,
    Arc<SharedWorkspace>,
    TcpListener,
    PathBuf,
) {
    let storage = tempfile::tempdir().expect("storage");
    let client = tempfile::tempdir().expect("client");
    let client_root = std::fs::canonicalize(client.path()).expect("canonical client root");
    let server_root = prod_code_gateway::workspace::server_workspace_path(
        storage.path(),
        &client_root.to_string_lossy(),
        None,
    );
    std::fs::create_dir_all(&server_root).expect("server workspace");
    let workspace = Arc::new(SharedWorkspace::new(
        server_root,
        "generic".to_string(),
        None,
        None,
        None,
        None,
    ));
    let manager = Arc::new(WorkspaceManager::new());
    manager.insert_ready_for_test(Arc::clone(&workspace)).await;
    let mut state = ServerState::new(storage.path().to_path_buf());
    state.workspace_manager = manager;
    let state = Arc::new(state);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind shared server");
    (storage, client, state, workspace, listener, client_root)
}

async fn connect_shared(
    address: std::net::SocketAddr,
    client_root: &std::path::Path,
) -> Framed<TcpStream, ProdCodeCodec> {
    let socket = TcpStream::connect(address)
        .await
        .expect("connect shared client");
    let receive_buffer: libc::c_int = 1024;
    let set = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_RCVBUF,
            (&receive_buffer as *const libc::c_int).cast(),
            std::mem::size_of::<libc::c_int>() as libc::socklen_t,
        )
    };
    assert_eq!(set, 0, "limit client receive buffer");
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            supported_versions: Some(vec![PROTOCOL_VERSION]),
            client_name: "shared-backpressure-test".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: client_root.to_string_lossy().into_owned(),
            preferred_engine: Some("generic".to_string()),
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: None,
        }))
        .await
        .expect("send shared handshake");
    let response = tokio::time::timeout(WAIT, framed.next())
        .await
        .expect("shared handshake deadline")
        .expect("shared handshake response")
        .expect("valid shared handshake frame");
    assert!(matches!(response, WireMessage::HandshakeResponse(_)));
    framed
}

async fn wait_for_writers(expected: usize) {
    tokio::time::timeout(WAIT, async {
        while ACTIVE_SHARED_OUTPUT_WRITERS.load(std::sync::atomic::Ordering::Relaxed) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "expected {expected} shared writers, found {}",
            ACTIVE_SHARED_OUTPUT_WRITERS.load(std::sync::atomic::Ordering::Relaxed)
        )
    });
}

async fn next_message(client: &mut Framed<TcpStream, ProdCodeCodec>) -> WireMessage {
    tokio::time::timeout(WAIT, client.next())
        .await
        .expect("shared response deadline")
        .expect("shared connection remains open")
        .expect("valid shared response")
}

async fn wait_for_large_response_header(client: &Framed<TcpStream, ProdCodeCodec>) {
    let deadline = tokio::time::Instant::now() + WAIT;
    let mut header = [0u8; 4];
    loop {
        let peeked = tokio::time::timeout_at(deadline, client.get_ref().peek(&mut header))
            .await
            .expect("large response header deadline")
            .expect("peek large response header");
        if peeked < header.len() {
            tokio::task::yield_now().await;
            continue;
        }
        let advertised = u32::from_be_bytes(header) as usize;
        assert!(
            advertised > 8 * 1024 * 1024,
            "large response advertises {advertised} bytes"
        );
        return;
    }
}

async fn next_response_id(
    client: &mut Framed<TcpStream, ProdCodeCodec>,
    expected: serde_json::Value,
) {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let WireMessage::LspPayload(raw) = tokio::time::timeout_at(deadline, client.next())
            .await
            .expect("shared response-id deadline")
            .expect("shared connection remains open")
            .expect("valid shared response")
        else {
            continue;
        };
        let response: serde_json::Value = serde_json::from_str(&raw).expect("JSON response");
        if response.get("id") == Some(&expected) {
            return;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unread_connected_client_cannot_hold_shared_handler_after_disconnect() {
    let (_storage, _client_root_owner, state, workspace, listener, client_root) =
        shared_server().await;
    let address = listener.local_addr().expect("shared address");
    let handler = OwnedHandler::spawn(listener, Arc::clone(&state));
    let mut client = connect_shared(address, &client_root).await;
    wait_for_writers(1).await;

    let huge_id = "x".repeat(8 * 1024 * 1024);
    client
        .feed(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": huge_id,
                "method": "initialize",
                "params": {}
            })
            .to_string(),
        ))
        .await
        .expect("queue large initialize request");
    client
        .flush()
        .await
        .expect("flush large initialize request");
    wait_for_large_response_header(&client).await;

    // A second client remains fully responsive while the first writer is blocked. Exercise both
    // control and ordinary finite LSP responses before cleanly draining its final response.
    let independent_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind independent server");
    let independent_address = independent_listener
        .local_addr()
        .expect("independent address");
    let independent_handler = OwnedHandler::spawn(independent_listener, Arc::clone(&state));
    let mut independent = connect_shared(independent_address, &client_root).await;
    wait_for_writers(2).await;
    independent
        .send(WireMessage::Ping)
        .await
        .expect("send independent ping");
    assert!(matches!(
        next_message(&mut independent).await,
        WireMessage::Pong
    ));
    independent
        .send(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 7,
                "method": "initialize",
                "params": {}
            })
            .to_string(),
        ))
        .await
        .expect("send finite initialize");
    next_response_id(&mut independent, serde_json::json!(7)).await;
    independent
        .send(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 8,
                "method": "shutdown",
                "params": null
            })
            .to_string(),
        ))
        .await
        .expect("send finite shutdown");
    next_response_id(&mut independent, serde_json::json!(8)).await;
    independent
        .send(WireMessage::Disconnect {
            reason: "independent client done".to_string(),
        })
        .await
        .expect("disconnect independent client");
    let independent_result = match independent_handler.finishes_within(WAIT).await {
        Ok(result) => result,
        Err(handler) => {
            handler.abort_and_join().await;
            panic!("independent handler exceeded teardown budget");
        }
    };
    assert!(
        independent_result.is_ok(),
        "independent session drains cleanly"
    );
    wait_for_writers(1).await;

    // Saturate the queue behind the blocked large frame. The first expired generation closes
    // every later sender, so repeated notifications cannot each extend the same dead session.
    for _ in 0..80 {
        client
            .feed(WireMessage::Ping)
            .await
            .expect("queue ping behind blocked output");
    }
    client
        .feed(WireMessage::Disconnect {
            reason: "test disconnect while output is blocked".to_string(),
        })
        .await
        .expect("queue disconnect");
    client
        .flush()
        .await
        .expect("flush requests to shared server");

    // Keep the peer connected and unread through the bounded-completion observation. Closing it
    // first would unblock the old writer and turn this regression into a false pass.
    let result = match handler.finishes_within(WAIT).await {
        Ok(result) => result,
        Err(handler) => {
            handler.abort_and_join().await;
            panic!("a connected unread peer retained the shared handler past its teardown budget");
        }
    };
    let error = result.expect_err("blocked shared output reports its deadline");
    assert!(
        format!("{error:#}").contains("deadline"),
        "blocked output error names its deadline: {error:#}"
    );
    wait_for_writers(0).await;
    assert_eq!(
        workspace
            .active_sessions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        state
            .active_sessions
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    drop(client);

    // With no further client input, the writer's own deadline still wakes the session loop and
    // retires the handler. This observes writer failure independently of Disconnect handling.
    let idle_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind idle writer server");
    let idle_address = idle_listener.local_addr().expect("idle writer address");
    let idle_handler = OwnedHandler::spawn(idle_listener, Arc::clone(&state));
    let mut idle_client = connect_shared(idle_address, &client_root).await;
    wait_for_writers(1).await;
    idle_client
        .send(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "z".repeat(8 * 1024 * 1024),
                "method": "initialize",
                "params": {}
            })
            .to_string(),
        ))
        .await
        .expect("send idle writer probe");
    let idle_result = match idle_handler.finishes_within(WAIT).await {
        Ok(result) => result,
        Err(handler) => {
            handler.abort_and_join().await;
            panic!("idle writer failure did not retire shared handler");
        }
    };
    assert!(
        format!(
            "{:#}",
            idle_result.expect_err("idle blocked writer reports failure")
        )
        .contains("deadline")
    );
    wait_for_writers(0).await;
    drop(idle_client);

    // A transport reset also retires an otherwise idle writer without waiting for another frame.
    let reset_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind reset server");
    let reset_address = reset_listener.local_addr().expect("reset address");
    let reset_handler = OwnedHandler::spawn(reset_listener, Arc::clone(&state));
    let reset_client = connect_shared(reset_address, &client_root).await;
    wait_for_writers(1).await;
    let reset_socket = reset_client.into_inner();
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    let set = unsafe {
        libc::setsockopt(
            reset_socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&linger as *const libc::linger).cast(),
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    assert_eq!(set, 0, "set reset-on-close");
    drop(reset_socket);
    match reset_handler.finishes_within(WAIT).await {
        Ok(_) => {}
        Err(handler) => {
            handler.abort_and_join().await;
            panic!("transport error did not retire shared handler");
        }
    }
    wait_for_writers(0).await;

    // Cancel the handler during another blocked write and observe the exact child writer retire
    // while its peer remains connected, before this test runtime begins shutdown.
    let cancel_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind cancellation server");
    let cancel_address = cancel_listener.local_addr().expect("cancellation address");
    let cancel_handler = OwnedHandler::spawn(cancel_listener, Arc::clone(&state));
    let mut cancel_client = connect_shared(cancel_address, &client_root).await;
    wait_for_writers(1).await;
    cancel_client
        .send(WireMessage::LspPayload(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": "y".repeat(8 * 1024 * 1024),
                "method": "initialize",
                "params": {}
            })
            .to_string(),
        ))
        .await
        .expect("queue cancellation probe");
    cancel_client
        .flush()
        .await
        .expect("flush cancellation probe");
    wait_for_large_response_header(&cancel_client).await;
    cancel_handler.abort_and_join().await;
    wait_for_writers(0).await;
    drop(cancel_client);
}
