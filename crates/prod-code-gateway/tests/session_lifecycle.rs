use futures_util::{SinkExt, StreamExt};
use prod_code_gateway::workspace::{SharedWorkspace, WorkspaceManager};
use prod_code_gateway::{ServerState, handle_client};
use prod_code_protocol::{HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage};
use std::future::Future;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::{AbortHandle, JoinHandle, JoinSet};
use tokio::time::{Duration, Instant};
use tokio_util::codec::Framed;

#[tokio::test]
async fn dropping_an_unregistered_acquisition_releases_its_workspace_count() {
    let manager = Arc::new(WorkspaceManager::new());
    let root = PathBuf::from("/synthetic/session-lifecycle");
    let workspace = Arc::new(SharedWorkspace::new(
        root.clone(),
        "text".to_string(),
        None,
        None,
        None,
        None,
    ));
    manager.insert_ready_for_test(Arc::clone(&workspace)).await;

    let acquisition = manager.get_or_load(&root, "text").await.unwrap();
    assert_eq!(workspace.active_sessions.load(Ordering::Relaxed), 1);
    drop(acquisition);
    assert_eq!(
        workspace.active_sessions.load(Ordering::Relaxed),
        0,
        "a canceled handshake must return its counted workspace acquisition"
    );
}

async fn wait_for_retirement(
    manager: &WorkspaceManager,
    workspace: &SharedWorkspace,
    worktree: &std::path::Path,
    sessions: usize,
    owners: usize,
) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if workspace.active_sessions.load(Ordering::Relaxed) == sessions
                && manager.worktree_owner_count_for_test(worktree) == owners
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("session ownership retired");
}

/// Owns cancellation until the join has actually finished, including early test failure.
struct OwnedTask<T> {
    task: Option<JoinHandle<T>>,
}

impl<T> Drop for OwnedTask<T> {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl<T: Send + 'static> OwnedTask<T> {
    fn spawn(task: impl Future<Output = T> + Send + 'static) -> Self {
        Self {
            task: Some(tokio::spawn(task)),
        }
    }

    async fn abort_and_join(mut self) {
        let task = self.task.as_mut().expect("owned task");
        task.abort();
        let result = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("aborted helper task joins");
        self.task.take();
        let Err(error) = result else {
            panic!("aborted helper task must not complete");
        };
        assert!(
            error.is_cancelled(),
            "aborted helper task reports cancellation"
        );
    }

    async fn join(mut self) -> T {
        // Keep the handle in self while awaiting: a timeout or canceled join must still abort it.
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            self.task.as_mut().expect("owned task"),
        )
        .await
        .expect("owned helper task joins");
        self.task.take();
        result.expect("owned helper task does not panic")
    }
}

struct RetirementSignal(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for RetirementSignal {
    fn drop(&mut self) {
        if let Some(retired) = self.0.take() {
            let _ = retired.send(());
        }
    }
}

#[tokio::test]
async fn dropping_a_helper_retires_its_task_before_runtime_shutdown() {
    let (started, ready) = tokio::sync::oneshot::channel();
    let (retired, retirement) = tokio::sync::oneshot::channel();
    let owner = OwnedTask::spawn(async move {
        let _retirement = RetirementSignal(Some(retired));
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    let abort = owner.task.as_ref().expect("owned task").abort_handle();
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .expect("helper starts")
        .expect("readiness sender remains owned");
    drop(owner);
    let retired_without_rescue = tokio::time::timeout(Duration::from_millis(100), retirement)
        .await
        .is_ok();
    // The RED must clean up the exact task too, before its assertion panics.
    abort.abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !abort.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact helper retired");
    assert!(
        retired_without_rescue,
        "dropping OwnedTask detached a live helper"
    );
}

#[tokio::test]
async fn canceling_a_join_retires_the_joined_helper() {
    let (started, ready) = tokio::sync::oneshot::channel();
    let (retired, retirement) = tokio::sync::oneshot::channel();
    let helper = OwnedTask::spawn(async move {
        let _retirement = RetirementSignal(Some(retired));
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    let abort = helper.task.as_ref().expect("owned task").abort_handle();
    let joining = OwnedTask::spawn(async move { helper.join().await });
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .expect("helper starts")
        .expect("readiness sender remains owned");
    joining.abort_and_join().await;
    let retired_without_rescue = tokio::time::timeout(Duration::from_secs(1), retirement)
        .await
        .is_ok();
    abort.abort();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !abort.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact joined helper retired");
    assert!(
        retired_without_rescue,
        "canceling a join detached its helper"
    );
}

/// The accept helper owns every accepted session through its JoinSet, rather than handing
/// detached handles to the test. The channel exposes only a precise abort capability.
struct OwnedSessionServer {
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    accept: OwnedTask<()>,
    aborts: tokio::sync::mpsc::UnboundedReceiver<AbortHandle>,
}

impl OwnedSessionServer {
    fn start(listener: TcpListener, state: Arc<ServerState>) -> Self {
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel();
        let (abort_tx, aborts) = tokio::sync::mpsc::unbounded_channel();
        let accept = OwnedTask::spawn(async move {
            let mut sessions = JoinSet::new();
            loop {
                tokio::select! {
                    joined = sessions.join_next(), if !sessions.is_empty() => {
                        let joined = joined.expect("owned accepted session exists");
                        if let Err(error) = joined {
                            assert!(error.is_cancelled(), "owned accepted session panicked");
                        }
                    }
                    accepted = listener.accept() => {
                        let Ok((socket, peer)) = accepted else {
                            break;
                        };
                        let state = Arc::clone(&state);
                        let abort = sessions.spawn(async move {
                            let _ = handle_client(socket, peer, state).await;
                        });
                        if abort_tx.send(abort.clone()).is_err() {
                            abort.abort();
                        }
                    }
                    _ = &mut shutdown_rx => {
                        sessions.abort_all();
                        break;
                    }
                }
            }
            while let Some(joined) = sessions.join_next().await {
                if let Err(error) = joined {
                    assert!(error.is_cancelled(), "owned accepted session panicked");
                }
            }
        });
        Self {
            shutdown: Some(shutdown_tx),
            accept,
            aborts,
        }
    }

    async fn next_task(&mut self) -> AbortHandle {
        tokio::time::timeout(Duration::from_secs(5), self.aborts.recv())
            .await
            .expect("accepted session task is registered before readiness")
            .expect("accept helper remains live")
    }

    async fn shutdown(mut self) {
        let _ = self.shutdown.take().expect("shutdown sender").send(());
        self.accept.join().await;
    }
}

#[tokio::test]
async fn dropping_the_accept_helper_retires_accepted_sessions() {
    let storage = tempfile::tempdir().expect("storage");
    let state = Arc::new(ServerState::new(storage.path().to_path_buf()));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("listener");
    let address = listener.local_addr().expect("address");
    let mut server = OwnedSessionServer::start(listener, state);
    let accept = server
        .accept
        .task
        .as_ref()
        .expect("accept task")
        .abort_handle();
    let client = TcpStream::connect(address).await.expect("client");
    let session = server.next_task().await;
    drop(server);
    let outcome = tokio::time::timeout(Duration::from_secs(1), async {
        while !accept.is_finished() || !session.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Retain exact capabilities for cleanup even if this regression fails.
    accept.abort();
    session.abort();
    drop(client);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !accept.is_finished() || !session.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("exact accept and session tasks retired");
    assert!(
        outcome.is_ok(),
        "dropping the accept helper detached accepted work"
    );
}

#[tokio::test]
async fn canceled_view_retires_exactly_itself_and_preserves_an_independent_owner() {
    let manager = Arc::new(WorkspaceManager::new());
    let root = PathBuf::from("/synthetic/shared-workspace");
    let worktree = PathBuf::from("/client/the-same-worktree");
    let workspace = Arc::new(SharedWorkspace::new(
        root.clone(),
        "text".to_string(),
        None,
        None,
        None,
        None,
    ));
    manager.insert_ready_for_test(Arc::clone(&workspace)).await;

    let first = manager
        .register_session_view(
            1,
            worktree.clone(),
            manager.get_or_load(&root, "text").await.unwrap(),
        )
        .await;
    let second = manager
        .register_session_view(
            2,
            worktree.clone(),
            manager.get_or_load(&root, "text").await.unwrap(),
        )
        .await;
    assert_eq!(workspace.active_sessions.load(Ordering::Relaxed), 2);
    assert_eq!(manager.worktree_owner_count_for_test(&worktree), 2);

    drop(first);
    wait_for_retirement(&manager, &workspace, &worktree, 1, 1).await;
    assert_eq!(second.session_id, 2, "the unrelated owner remains live");

    manager.unregister_session_view(second).await;
    wait_for_retirement(&manager, &workspace, &worktree, 0, 0).await;
}

async fn fake_generic_workspace(root: &std::path::Path) -> Arc<SharedWorkspace> {
    let script = root.join("language-server.py");
    std::fs::write(
        &script,
        r#"import json, sys
def read():
    length = 0
    while True:
        line = sys.stdin.buffer.readline()
        if not line: return None
        line = line.strip()
        if not line: break
        if line.lower().startswith(b"content-length:"): length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length)) if length else None
def send(value):
    body = json.dumps(value).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
while True:
    message = read()
    if message is None: break
    if message.get("method") == "initialize":
        send({"jsonrpc":"2.0", "id":message["id"], "result":{"capabilities":{}}})
"#,
    )
    .unwrap();
    // GenericLspEngine owns its spawned child and configures Command::kill_on_drop(true).
    // Retaining this engine in the workspace keeps that child ownership explicit in the test.
    let owned_engine = prod_code_engine_generic::GenericLspEngine::spawn(
        root,
        prod_code_engine_generic::GenericLspConfig {
            command: "python3".to_string(),
            args: vec![script.to_string_lossy().into_owned()],
            ..Default::default()
        },
    )
    .await
    .expect("fake generic engine starts");
    Arc::new(SharedWorkspace::new(
        root.to_path_buf(),
        "python".to_string(),
        None,
        None,
        Some(Arc::new(owned_engine)),
        None,
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn validation_wait_cancellation_and_failed_response_release_every_owner() {
    let storage = tempfile::tempdir().unwrap();
    let client = tempfile::tempdir().unwrap();
    let client_root = std::fs::canonicalize(client.path()).unwrap();
    let server_root = prod_code_gateway::workspace::server_workspace_path(
        storage.path(),
        &client_root.to_string_lossy(),
        None,
    );
    std::fs::create_dir_all(&server_root).unwrap();
    let workspace = fake_generic_workspace(&server_root).await;
    let validation_gate = Arc::clone(&workspace.generic_validation_session)
        .lock_owned()
        .await;
    let manager = Arc::new(WorkspaceManager::new());
    manager.insert_ready_for_test(Arc::clone(&workspace)).await;
    let mut state = ServerState::new(storage.path().to_path_buf());
    state.workspace_manager = Arc::clone(&manager);
    let state = Arc::new(state);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_state = Arc::clone(&state);
    let server = OwnedTask::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, server_state).await
    });
    let mut client = Framed::new(
        TcpStream::connect(addr).await.unwrap(),
        ProdCodeCodec::new(),
    );
    client
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            supported_versions: Some(vec![PROTOCOL_VERSION]),
            client_name: "session-lifecycle-test".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: client_root.to_string_lossy().into_owned(),
            preferred_engine: Some("python".to_string()),
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: Some(prod_code_protocol::PURPOSE_VALIDATION.to_string()),
        }))
        .await
        .unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if state.active_sessions.load(Ordering::Relaxed) == 1
                && workspace.active_sessions.load(Ordering::Relaxed) == 1
                && manager.worktree_owner_count_for_test(&client_root) == 1
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("handshake reached the held validation lock");

    server.abort_and_join().await;
    drop(client);
    drop(validation_gate);
    wait_for_retirement(&manager, &workspace, &client_root, 0, 0).await;
    assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0);

    // Repeat the same attachment, but let validation finish after the peer has reset its
    // socket. The fallible HandshakeResponse write must take the identical retirement path.
    let validation_gate = Arc::clone(&workspace.generic_validation_session)
        .lock_owned()
        .await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server_state = Arc::clone(&state);
    let server = OwnedTask::spawn(async move {
        let (socket, peer) = listener.accept().await.unwrap();
        handle_client(socket, peer, server_state).await
    });
    let mut client = Framed::new(
        TcpStream::connect(addr).await.unwrap(),
        ProdCodeCodec::new(),
    );
    client
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            supported_versions: Some(vec![PROTOCOL_VERSION]),
            client_name: "session-lifecycle-test".to_string(),
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: client_root.to_string_lossy().into_owned(),
            preferred_engine: Some("python".to_string()),
            base_workspace_name: None,
            engine_subpath: None,
            client_agent: None,
            client_host: None,
            purpose: Some(prod_code_protocol::PURPOSE_VALIDATION.to_string()),
        }))
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while workspace.active_sessions.load(Ordering::Relaxed) != 1
            || manager.worktree_owner_count_for_test(&client_root) != 1
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("second handshake reached the held validation lock");
    let socket = client.into_inner();
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    let set = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            (&linger as *const libc::linger).cast(),
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    assert_eq!(set, 0, "set reset-on-close");
    drop(socket);
    drop(validation_gate);
    let response = server.join().await;
    assert!(response.is_err(), "the reset handshake write must fail");
    wait_for_retirement(&manager, &workspace, &client_root, 0, 0).await;
    assert_eq!(state.active_sessions.load(Ordering::Relaxed), 0);
}

struct WireSession {
    framed: Framed<TcpStream, ProdCodeCodec>,
    next_id: u64,
    root: PathBuf,
}

impl WireSession {
    async fn open(addr: std::net::SocketAddr, root: PathBuf) -> Self {
        let mut framed = Framed::new(
            TcpStream::connect(addr).await.unwrap(),
            ProdCodeCodec::new(),
        );
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(vec![PROTOCOL_VERSION]),
                client_name: "session-lifecycle-real-engine".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: root.to_string_lossy().into_owned(),
                preferred_engine: Some("rust".to_string()),
                base_workspace_name: Some("session-lifecycle-real".to_string()),
                engine_subpath: None,
                client_agent: None,
                client_host: None,
                purpose: None,
            }))
            .await
            .unwrap();
        let response = tokio::time::timeout(std::time::Duration::from_secs(120), framed.next())
            .await
            .expect("real engine handshake completes");
        assert!(matches!(
            response,
            Some(Ok(WireMessage::HandshakeResponse(_)))
        ));
        Self {
            framed,
            next_id: 1,
            root,
        }
    }

    fn uri(&self) -> String {
        format!("file://{}/src/lib.rs", self.root.display())
    }

    async fn open_text(&mut self, text: &str) {
        self.framed
            .send(WireMessage::LspPayload(
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "method": "textDocument/didOpen",
                    "params": { "textDocument": {
                        "uri": self.uri(), "languageId": "rust", "version": 1, "text": text
                    }}
                })
                .to_string(),
            ))
            .await
            .unwrap();
    }

    async fn symbols(&mut self) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        self.framed
            .send(WireMessage::LspPayload(
                serde_json::json!({
                    "jsonrpc": "2.0", "id": id,
                    "method": "textDocument/documentSymbol",
                    "params": { "textDocument": { "uri": self.uri() } }
                })
                .to_string(),
            ))
            .await
            .unwrap();
        // Notifications can arrive before the response. Keep one deadline for the complete
        // request instead of granting every irrelevant frame another two minutes.
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let frame = tokio::time::timeout(
                deadline.saturating_duration_since(Instant::now()),
                self.framed.next(),
            )
            .await
            .expect("document symbols answer before its original deadline");
            let Some(Ok(WireMessage::LspPayload(raw))) = frame else {
                panic!("session ended before document symbols: {frame:?}");
            };
            let value: serde_json::Value = serde_json::from_str(&raw).unwrap();
            if value.get("id") == Some(&serde_json::json!(id)) {
                return value;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn canceled_real_rust_session_restores_overlay_without_retiring_another_owner() {
    let storage = tempfile::tempdir().unwrap();
    let client = tempfile::tempdir().unwrap();
    let client_root = std::fs::canonicalize(client.path()).unwrap();
    let server_root = prod_code_gateway::workspace::server_workspace_path(
        storage.path(),
        &client_root.to_string_lossy(),
        Some("session-lifecycle-real"),
    );
    std::fs::create_dir_all(server_root.join("src")).unwrap();
    std::fs::write(
        server_root.join("Cargo.toml"),
        "[package]\nname = \"session-lifecycle-real\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(
        server_root.join("src/lib.rs"),
        "pub fn disk_symbol() -> u32 { 7 }\n",
    )
    .unwrap();

    let state = Arc::new(ServerState::new(storage.path().to_path_buf()));
    let manager = Arc::clone(&state.workspace_manager);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let mut server = OwnedSessionServer::start(listener, Arc::clone(&state));

    let mut keeper = WireSession::open(addr, client_root.clone()).await;
    let _keeper_task = server.next_task().await;
    let mut canceled = WireSession::open(addr, client_root.clone()).await;
    let canceled_task = server.next_task().await;
    canceled
        .open_text("pub fn overlay_symbol() -> u32 { 99 }\n")
        .await;
    let overlaid = canceled.symbols().await.to_string();
    assert!(overlaid.contains("overlay_symbol"), "{overlaid}");

    canceled_task.abort();
    drop(canceled);
    let loaded = manager.get_loaded(&server_root).await.unwrap();
    wait_for_retirement(&manager, &loaded, &client_root, 1, 1).await;
    assert_eq!(state.active_sessions.load(Ordering::Relaxed), 1);

    let kept = keeper.symbols().await.to_string();
    assert!(kept.contains("disk_symbol"), "{kept}");
    assert!(!kept.contains("overlay_symbol"), "{kept}");
    drop(keeper);
    wait_for_retirement(&manager, &loaded, &client_root, 0, 0).await;

    let mut fresh = WireSession::open(addr, client_root.clone()).await;
    let _fresh_task = server.next_task().await;
    let fresh_symbols = fresh.symbols().await.to_string();
    assert!(fresh_symbols.contains("disk_symbol"), "{fresh_symbols}");
    assert!(!fresh_symbols.contains("overlay_symbol"), "{fresh_symbols}");
    drop(fresh);
    wait_for_retirement(&manager, &loaded, &client_root, 0, 0).await;
    server.shutdown().await;
}
