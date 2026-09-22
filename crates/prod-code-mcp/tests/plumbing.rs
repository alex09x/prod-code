//! The client side of the wire protocol that `orchestration.rs` never touches: running a
//! remote command (`exec`), ranking declarations by intent (`search`), reading a file that
//! lives only on the gateway host (`remote_fs`), and the pooled LSP session that batch features
//! share (`session`) — its document bookkeeping, its refresh after a local edit, and its retry
//! when a pooled connection has died under it.
//!
//! `exec`, `search` and `remote_fs` speak a request/response pair the scripted gateway in
//! `prod-code-testkit` does not answer (it only knows the pre-flight sync, the handshake and
//! LSP), so the gateways here are minimal, purpose-built ones: accept one connection, answer
//! (or misbehave with) exactly the message the test needs. A workspace with one tracked file
//! sends a manifest probe before its real request — `expect_after_sync` answers that probe (as
//! "nothing missing", the same fiction `ScriptedGateway` uses) and hands back the message that
//! follows it.
//!
//! `session` reuses `Workspace`/`answers` but records every JSON-RPC message a session sends,
//! since that is what its own bookkeeping (open once, `didChange` on refresh, `didClose` on
//! close) can be checked against. Every one of its assertions follows a request the session
//! waited on (`query`, `refresh`) or the gateway's own disconnect notice, never a bare
//! notification: a notification is only ever pushed onto the wire, and checking for its effect
//! right after sending it would race the gateway's own read of it.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::session::LspSession;
use prod_code_protocol::{
    ExecChanges, ExecChunk, ExecExit, FileDelta, HandshakeResponse, PROTOCOL_VERSION,
    ProdCodeCodec, ReadFileResponse, SearchHit, SearchResponse, SyncProbeResponse, SyncResponse,
    WireMessage,
};
use prod_code_testkit::{Answer, Workspace, answers};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Notify;
use tokio_util::codec::Framed;

async fn accept_one(listener: TcpListener) -> Framed<TcpStream, ProdCodeCodec> {
    let (socket, _) = listener.accept().await.expect("a connection arrives");
    Framed::new(socket, ProdCodeCodec::new())
}

/// Answers a manifest probe or a watermark delta exactly like `ScriptedGateway` does ("nothing
/// missing"), and returns the first message that is neither — the actual request the test
/// cares about.
async fn expect_after_sync(framed: &mut Framed<TcpStream, ProdCodeCodec>) -> WireMessage {
    loop {
        let message = framed
            .next()
            .await
            .expect("a message arrives")
            .expect("it decodes");
        match message {
            WireMessage::SyncProbeRequest(req) => {
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: Vec::new(),
                    }))
                    .await
                    .unwrap();
            }
            WireMessage::SyncRequest(req) => {
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 0,
                        workspace_was_fresh: false,
                    }))
                    .await
                    .unwrap();
            }
            other => return other,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// exec: running a command on the gateway (`crate::exec::run_remote`)
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn run_remote_streams_output_and_writes_back_pulled_files() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let exec_req = match expect_after_sync(&mut framed).await {
            WireMessage::ExecRequest(req) => req,
            other => panic!("expected an ExecRequest: {other:?}"),
        };
        assert_eq!(exec_req.command, vec!["echo".to_string(), "hi".to_string()]);
        assert!(exec_req.pull_changes);
        framed
            .send(WireMessage::ExecChunk(ExecChunk {
                stderr: false,
                data: Some(b"out\n".to_vec()),
            }))
            .await
            .unwrap();
        framed
            .send(WireMessage::ExecChunk(ExecChunk {
                stderr: true,
                data: Some(b"warn\n".to_vec()),
            }))
            .await
            .unwrap();
        framed
            .send(WireMessage::ExecChanges(ExecChanges {
                files: vec![FileDelta {
                    relative_path: "generated/out.txt".to_string(),
                    content: Some(b"built\n".to_vec()),
                    is_executable: false,
                }],
            }))
            .await
            .unwrap();
        framed
            .send(WireMessage::ExecExit(ExecExit {
                exit_code: Some(0),
                duration_ms: 42,
                server_workspace_root: "/srv/ws".to_string(),
                timed_out: false,
                error: None,
            }))
            .await
            .unwrap();
    });

    let mut captured = Vec::new();
    let outcome = prod_code_mcp::exec::run_remote(
        addr,
        &root,
        None,
        vec!["echo".to_string(), "hi".to_string()],
        Vec::new(),
        0,
        true,
        |stderr, data| captured.push((stderr, data.to_vec())),
    )
    .await
    .expect("the command runs");

    assert_eq!(
        captured,
        vec![(false, b"out\n".to_vec()), (true, b"warn\n".to_vec())]
    );
    assert_eq!(outcome.exit.exit_code, Some(0));
    assert_eq!(outcome.exit.server_workspace_root, "/srv/ws");
    assert!(!outcome.exit.timed_out);
    assert_eq!(outcome.pulled_files, vec!["generated/out.txt".to_string()]);
    assert_eq!(ws.read("generated/out.txt"), "built\n");
}

#[tokio::test]
async fn run_remote_refuses_an_empty_command_without_connecting() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    // Never dialed: an empty command is refused before the gateway is contacted.
    let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let err = prod_code_mcp::exec::run_remote(addr, &root, None, Vec::new(), Vec::new(), 0, false, |_, _| {})
        .await
        .expect_err("an empty command is refused");
    let text = format!("{err:#}");
    assert!(text.contains("empty command"), "{text}");
}

#[tokio::test]
async fn run_remote_reports_an_unexpected_message() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = expect_after_sync(&mut framed).await; // the ExecRequest
        framed.send(WireMessage::Ping).await.unwrap();
    });

    let err = prod_code_mcp::exec::run_remote(
        addr,
        &root,
        None,
        vec!["true".to_string()],
        Vec::new(),
        0,
        false,
        |_, _| {},
    )
    .await
    .expect_err("a message that is neither a chunk nor an exit is refused");
    let text = format!("{err:#}");
    assert!(text.contains("unexpected message during exec"), "{text}");
}

#[tokio::test]
async fn run_remote_reports_the_gateway_closing_mid_command() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = expect_after_sync(&mut framed).await; // the ExecRequest, then nothing
    });

    let err = prod_code_mcp::exec::run_remote(
        addr,
        &root,
        None,
        vec!["true".to_string()],
        Vec::new(),
        0,
        false,
        |_, _| {},
    )
    .await
    .expect_err("a gateway that hangs up mid-command is refused");
    let text = format!("{err:#}");
    assert!(
        text.contains("gateway closed the connection during exec"),
        "{text}"
    );
}

#[tokio::test]
async fn run_remote_fails_to_connect_to_an_unreachable_gateway() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    // A privileged port nothing listens on without root: unlike binding an ephemeral port and
    // dropping it, this cannot be raced by another test's own `bind("127.0.0.1:0")` grabbing
    // the same number before this connects.
    let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let err = prod_code_mcp::exec::run_remote(
        addr,
        &root,
        None,
        vec!["true".to_string()],
        Vec::new(),
        0,
        false,
        |_, _| {},
    )
    .await
    .expect_err("connecting to a closed port fails");
    let text = format!("{err:#}");
    assert!(text.contains("failed to connect to remote gateway"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// search: ranking declarations by intent (`crate::search::search`)
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn search_returns_the_gateways_hits() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let req = match expect_after_sync(&mut framed).await {
            WireMessage::SearchRequest(req) => req,
            other => panic!("expected a SearchRequest: {other:?}"),
        };
        assert_eq!(req.query, "who runs a workspace");
        framed
            .send(WireMessage::SearchResponse(SearchResponse {
                server_workspace_root: "/srv/ws".to_string(),
                hits: vec![SearchHit {
                    file: "src/place.rs".to_string(),
                    line: 9,
                    kind: "function".to_string(),
                    name: "place".to_string(),
                    container: None,
                    signature: "fn place()".to_string(),
                    doc: String::new(),
                }],
                indexed_files: 3,
                indexed_declarations: 12,
                took_ms: 4,
                error: None,
            }))
            .await
            .unwrap();
    });

    let resp = prod_code_mcp::search::search(addr, &root, "who runs a workspace", 5, None)
        .await
        .expect("the search runs");
    assert_eq!(resp.hits.len(), 1);
    assert_eq!(resp.hits[0].name, "place");
    assert_eq!(resp.indexed_declarations, 12);
}

#[tokio::test]
async fn search_refuses_an_empty_query_without_connecting() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let addr: SocketAddr = "127.0.0.1:1".parse().unwrap();

    let err = prod_code_mcp::search::search(addr, &root, "   ", 0, None)
        .await
        .expect_err("an empty query is refused");
    let text = format!("{err:#}");
    assert!(text.contains("empty query"), "{text}");
}

#[tokio::test]
async fn search_reports_when_the_gateway_refuses_the_query() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = expect_after_sync(&mut framed).await;
        framed
            .send(WireMessage::SearchResponse(SearchResponse {
                server_workspace_root: "/srv/ws".to_string(),
                hits: Vec::new(),
                indexed_files: 0,
                indexed_declarations: 0,
                took_ms: 0,
                error: Some("index not ready".to_string()),
            }))
            .await
            .unwrap();
    });

    let err = prod_code_mcp::search::search(addr, &root, "anything", 0, None)
        .await
        .expect_err("a gateway error is surfaced");
    let text = format!("{err:#}");
    assert!(text.contains("search refused: index not ready"), "{text}");
}

#[tokio::test]
async fn search_reports_an_unexpected_message() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = expect_after_sync(&mut framed).await;
        framed.send(WireMessage::Ping).await.unwrap();
    });

    let err = prod_code_mcp::search::search(addr, &root, "anything", 0, None)
        .await
        .expect_err("a message that is not a SearchResponse is refused");
    let text = format!("{err:#}");
    assert!(text.contains("unexpected message during search"), "{text}");
}

#[tokio::test]
async fn search_reports_the_gateway_closing_the_connection() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = expect_after_sync(&mut framed).await; // the SearchRequest, then nothing
    });

    let err = prod_code_mcp::search::search(addr, &root, "anything", 0, None)
        .await
        .expect_err("a gateway that hangs up is refused");
    let text = format!("{err:#}");
    assert!(
        text.contains("gateway closed the connection during the search"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------------------------
// remote_fs: reading a file that lives only on the gateway host
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn read_remote_file_returns_the_bytes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let req = match framed.next().await {
            Some(Ok(WireMessage::ReadFileRequest(req))) => req,
            other => panic!("expected a ReadFileRequest: {other:?}"),
        };
        framed
            .send(WireMessage::ReadFileResponse(ReadFileResponse {
                path: req.path,
                content: Some(b"content".to_vec()),
                truncated: false,
                error: None,
            }))
            .await
            .unwrap();
    });

    let (bytes, truncated) = prod_code_mcp::remote_fs::read_remote_file(addr, "/usr/include/stdlib.h", 4096)
        .await
        .expect("the file reads");
    assert_eq!(bytes, b"content");
    assert!(!truncated);
}

#[tokio::test]
async fn read_remote_file_reports_the_gateways_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let req = match framed.next().await {
            Some(Ok(WireMessage::ReadFileRequest(req))) => req,
            other => panic!("expected a ReadFileRequest: {other:?}"),
        };
        framed
            .send(WireMessage::ReadFileResponse(ReadFileResponse {
                path: req.path,
                content: None,
                truncated: false,
                error: Some("no such file".to_string()),
            }))
            .await
            .unwrap();
    });

    let err = prod_code_mcp::remote_fs::read_remote_file(addr, "/missing", 4096)
        .await
        .expect_err("the gateway's error is surfaced");
    let text = format!("{err:#}");
    assert!(text.contains("no such file"), "{text}");
}

#[tokio::test]
async fn read_remote_file_reports_an_empty_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let req = match framed.next().await {
            Some(Ok(WireMessage::ReadFileRequest(req))) => req,
            other => panic!("expected a ReadFileRequest: {other:?}"),
        };
        framed
            .send(WireMessage::ReadFileResponse(ReadFileResponse {
                path: req.path,
                content: None,
                truncated: false,
                error: None,
            }))
            .await
            .unwrap();
    });

    let err = prod_code_mcp::remote_fs::read_remote_file(addr, "/nothing", 4096)
        .await
        .expect_err("neither content nor an error is a bug in the gateway");
    let text = format!("{err:#}");
    assert!(text.contains("empty reply"), "{text}");
}

#[tokio::test]
async fn read_remote_file_reports_an_unexpected_reply() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = framed.next().await;
        framed.send(WireMessage::Pong).await.unwrap();
    });

    let err = prod_code_mcp::remote_fs::read_remote_file(addr, "/x", 4096)
        .await
        .expect_err("a message that is not a ReadFileResponse is refused");
    let text = format!("{err:#}");
    assert!(text.contains("unexpected reply"), "{text}");
}

#[tokio::test]
async fn read_remote_file_reports_the_gateway_closing_the_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut framed = accept_one(listener).await;
        let _ = framed.next().await; // the ReadFileRequest, then nothing
    });

    let err = prod_code_mcp::remote_fs::read_remote_file(addr, "/x", 4096)
        .await
        .expect_err("a gateway that hangs up is refused");
    let text = format!("{err:#}");
    assert!(text.contains("gateway closed the connection"), "{text}");
}

// ---------------------------------------------------------------------------------------------
// session: the pooled gateway session (`crate::session`)
// ---------------------------------------------------------------------------------------------

/// Like `ScriptedGateway`, but it also records every JSON-RPC message it receives — requests
/// and notifications alike — so a test can check what a session actually sent: a document
/// opened once for two queries, a `didChange` on refresh, a `didClose` from `close()`. Every
/// connection also raises `disconnected` once its own task ends, so a test that ends with a
/// notification (`close()` sends only notifications, then disconnects) has something to wait
/// on before it looks at what was recorded.
struct RecordingGateway {
    addr: SocketAddr,
    events: Arc<Mutex<Vec<serde_json::Value>>>,
    connections: Arc<AtomicUsize>,
    disconnected: Arc<Notify>,
}

impl RecordingGateway {
    async fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let events = Arc::new(Mutex::new(Vec::new()));
        let connections = Arc::new(AtomicUsize::new(0));
        let disconnected = Arc::new(Notify::new());
        let events_task = Arc::clone(&events);
        let connections_task = Arc::clone(&connections);
        let disconnected_task = Arc::clone(&disconnected);
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                connections_task.fetch_add(1, Ordering::SeqCst);
                let answer = Arc::clone(&answer);
                let events = Arc::clone(&events_task);
                let disconnected = Arc::clone(&disconnected_task);
                tokio::spawn(async move {
                    let _ = Self::serve(socket, answer, events).await;
                    disconnected.notify_one();
                });
            }
        });
        Self {
            addr,
            events,
            connections,
            disconnected,
        }
    }

    fn addr(&self) -> SocketAddr {
        self.addr
    }

    fn methods(&self) -> Vec<String> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| e.get("method").and_then(|m| m.as_str()).map(str::to_string))
            .collect()
    }

    fn events_for(&self, method: &str) -> Vec<serde_json::Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.get("method").and_then(|m| m.as_str()) == Some(method))
            .cloned()
            .collect()
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    /// Waits for a connection to end. `Notify` keeps one permit even when `notify_one()` ran
    /// before this was called, so this is race-free after `session.close()`, which only ever
    /// sends notifications (no response to wait on) before dropping the connection.
    async fn wait_for_a_disconnect(&self) {
        self.disconnected.notified().await;
    }

    async fn serve(
        socket: TcpStream,
        answer: Answer,
        events: Arc<Mutex<Vec<serde_json::Value>>>,
    ) -> anyhow::Result<()> {
        let mut framed = Framed::new(socket, ProdCodeCodec::new());
        while let Some(message) = framed.next().await {
            match message? {
                WireMessage::SyncProbeRequest(req) => {
                    framed
                        .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                            server_workspace_root: req.client_workspace_root.clone(),
                            seeded: false,
                            files_deleted: 0,
                            missing: Vec::new(),
                        }))
                        .await?;
                }
                WireMessage::SyncRequest(req) => {
                    framed
                        .send(WireMessage::SyncResponse(SyncResponse {
                            server_workspace_root: req.client_workspace_root.clone(),
                            files_updated: 0,
                            files_deleted: 0,
                            bytes_transferred: 0,
                            duration_ms: 0,
                            workspace_was_fresh: false,
                        }))
                        .await?;
                }
                WireMessage::HandshakeRequest(req) => {
                    framed
                        .send(WireMessage::HandshakeResponse(HandshakeResponse {
                            protocol_version: PROTOCOL_VERSION,
                            server_pid: std::process::id(),
                            session_id: 1,
                            server_workspace_root: req.client_workspace_root.clone(),
                            detected_engine: "rust".to_string(),
                        }))
                        .await?;
                }
                WireMessage::LspPayload(json) => {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                        continue;
                    };
                    events.lock().unwrap().push(value.clone());
                    let Some(id) = value.get("id").cloned() else {
                        continue; // a notification: didOpen, didChange, didClose, initialized
                    };
                    let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                    let params = value
                        .get("params")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let result = if method == "initialize" {
                        serde_json::json!({ "capabilities": { "hoverProvider": true } })
                    } else {
                        answer(method, &params)
                    };
                    let response = serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                    framed
                        .send(WireMessage::LspPayload(response.to_string()))
                        .await?;
                }
                WireMessage::Disconnect { .. } => break,
                _ => {}
            }
        }
        Ok(())
    }
}

fn hover_answer() -> Answer {
    Arc::new(|method, _params| match method {
        "textDocument/hover" => answers::hover("hover text"),
        _ => serde_json::Value::Null,
    })
}

/// Queries `file` for hover and discards the result: a request/response round trip, used only
/// to prove that everything sent before it (typically a notification) was already read by the
/// gateway, since frames on one connection are delivered in the order they were sent.
async fn sync_barrier(session: &mut LspSession, file: &std::path::Path) {
    let uri = session.uri_for(file).unwrap();
    let _ = session
        .query(
            file,
            "textDocument/hover",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn open_reports_the_gateways_engine_and_the_canonical_root() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let gateway = RecordingGateway::start(hover_answer()).await;

    let session = LspSession::open(gateway.addr(), &root, None)
        .await
        .expect("the session opens");
    assert_eq!(session.engine, "rust");
    assert_eq!(session.root(), root.as_path());
}

#[tokio::test]
async fn query_opens_the_document_once_for_repeated_queries() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&file))
        .await
        .unwrap();
    let uri = session.uri_for(&file).unwrap();
    let params = serde_json::json!({ "textDocument": { "uri": uri }, "position": { "line": 0, "character": 0 } });
    for _ in 0..2 {
        let result = session
            .query(&file, "textDocument/hover", params.clone())
            .await
            .unwrap();
        assert_eq!(result["contents"]["value"], "hover text");
    }
    session.close().await;
    gateway.wait_for_a_disconnect().await;

    let methods = gateway.methods();
    assert_eq!(
        methods.iter().filter(|m| *m == "textDocument/didOpen").count(),
        1,
        "{methods:?}"
    );
    assert_eq!(
        methods
            .iter()
            .filter(|m| *m == "textDocument/hover")
            .count(),
        2,
        "{methods:?}"
    );
    assert_eq!(
        methods
            .iter()
            .filter(|m| *m == "textDocument/didClose")
            .count(),
        1,
        "close() ends every document it opened: {methods:?}"
    );
}

#[tokio::test]
async fn query_with_text_opens_a_private_overlay_instead_of_the_file_on_disk() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(Arc::new(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&file))
        .await
        .unwrap();
    let uri = session.uri_for(&file).unwrap();
    let overlay = "pub fn b() {}\n";
    let _ = session
        .query_with_text(
            &file,
            overlay,
            "textDocument/diagnostic",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await
        .unwrap();

    let opens = gateway.events_for("textDocument/didOpen");
    assert_eq!(opens.len(), 1, "{opens:?}");
    assert_eq!(opens[0]["params"]["textDocument"]["text"], overlay);
    // Nothing was written: the overlay lives only in the session.
    assert_eq!(ws.read("src/lib.rs"), "pub fn a() {}\n");
}

#[tokio::test]
async fn open_text_sends_a_did_change_for_a_document_already_open() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&file))
        .await
        .unwrap();
    // Opens it first.
    sync_barrier(&mut session, &file).await;
    // Now upgrades it in place.
    session
        .open_text(&file, "pub fn changed() {}\n")
        .await
        .unwrap();
    // Only a request/response round trip proves the gateway has read the `didChange` above.
    sync_barrier(&mut session, &file).await;

    assert_eq!(gateway.events_for("textDocument/didOpen").len(), 1);
    let changes = gateway.events_for("textDocument/didChange");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0]["params"]["contentChanges"][0]["text"],
        "pub fn changed() {}\n"
    );
}

#[tokio::test]
async fn open_text_opens_a_document_that_was_never_queried() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&file))
        .await
        .unwrap();
    session
        .open_text(&file, "pub fn fresh() {}\n")
        .await
        .unwrap();
    sync_barrier(&mut session, &file).await;

    let opens = gateway.events_for("textDocument/didOpen");
    assert_eq!(opens.len(), 1, "{opens:?}");
    assert_eq!(opens[0]["params"]["textDocument"]["text"], "pub fn fresh() {}\n");
    assert!(gateway.events_for("textDocument/didChange").is_empty());
}

#[tokio::test]
async fn refresh_sends_a_did_change_for_an_edit_made_after_the_document_was_opened() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&file))
        .await
        .unwrap();
    sync_barrier(&mut session, &file).await;

    ws.write("src/lib.rs", "pub fn edited() {}\n"); // dirty, uncommitted
    session.refresh().await.expect("refresh runs");
    sync_barrier(&mut session, &file).await;

    let changes = gateway.events_for("textDocument/didChange");
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert_eq!(
        changes[0]["params"]["contentChanges"][0]["text"],
        "pub fn edited() {}\n"
    );
}

#[tokio::test]
async fn refresh_closes_the_document_for_a_file_deleted_after_it_was_opened() {
    let ws = Workspace::new(&[
        ("src/gone.rs", "pub fn a() {}\n"),
        ("src/lib.rs", "pub fn keep() {}\n"),
    ]);
    let root = ws.root();
    let file = ws.path("src/gone.rs");
    let other = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&file))
        .await
        .unwrap();
    sync_barrier(&mut session, &file).await;

    std::fs::remove_file(&file).unwrap();
    session.refresh().await.expect("refresh runs");
    // `file` no longer exists, so the barrier goes through a document that still does.
    sync_barrier(&mut session, &other).await;

    assert_eq!(gateway.events_for("textDocument/didClose").len(), 1);
}

#[tokio::test]
async fn close_sends_a_did_close_for_every_document_it_opened() {
    let ws = Workspace::new(&[
        ("src/lib.rs", "pub fn a() {}\n"),
        ("src/other.rs", "pub fn b() {}\n"),
    ]);
    let root = ws.root();
    let a = ws.path("src/lib.rs");
    let b = ws.path("src/other.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    let mut session = LspSession::open(gateway.addr(), &root, Some(&a))
        .await
        .unwrap();
    for file in [&a, &b] {
        sync_barrier(&mut session, file).await;
    }
    session.close().await;
    gateway.wait_for_a_disconnect().await;

    assert_eq!(gateway.events_for("textDocument/didClose").len(), 2);
}

#[tokio::test]
async fn pooled_query_reuses_the_same_connection_for_a_second_query() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");
    let gateway = RecordingGateway::start(hover_answer()).await;

    for _ in 0..2 {
        let result = prod_code_mcp::session::pooled_query(
            gateway.addr(),
            &root,
            &file,
            "textDocument/hover",
            serde_json::json!({ "textDocument": { "uri": format!("file://{}", file.display()) } }),
        )
        .await
        .expect("the pooled query runs");
        assert_eq!(result["contents"]["value"], "hover text");
    }

    assert_eq!(
        gateway.connections(),
        1,
        "a second query on the same root reuses the pooled session"
    );
}

#[tokio::test]
async fn pooled_query_reopens_after_the_pooled_connection_dies() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
    let root = ws.root();
    let file = ws.path("src/lib.rs");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let attempts_task = Arc::clone(&attempts);
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let attempt = attempts_task.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut framed = Framed::new(socket, ProdCodeCodec::new());
                while let Some(Ok(message)) = framed.next().await {
                    match message {
                        WireMessage::SyncProbeRequest(req) => {
                            let _ = framed
                                .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    seeded: false,
                                    files_deleted: 0,
                                    missing: Vec::new(),
                                }))
                                .await;
                        }
                        WireMessage::SyncRequest(req) => {
                            let _ = framed
                                .send(WireMessage::SyncResponse(SyncResponse {
                                    server_workspace_root: req.client_workspace_root,
                                    files_updated: 0,
                                    files_deleted: 0,
                                    bytes_transferred: 0,
                                    duration_ms: 0,
                                    workspace_was_fresh: false,
                                }))
                                .await;
                        }
                        WireMessage::HandshakeRequest(req) => {
                            let _ = framed
                                .send(WireMessage::HandshakeResponse(HandshakeResponse {
                                    protocol_version: PROTOCOL_VERSION,
                                    server_pid: std::process::id(),
                                    session_id: 1,
                                    server_workspace_root: req.client_workspace_root,
                                    detected_engine: "rust".to_string(),
                                }))
                                .await;
                        }
                        WireMessage::LspPayload(json) => {
                            let Ok(value) = serde_json::from_str::<serde_json::Value>(&json)
                            else {
                                continue;
                            };
                            let Some(id) = value.get("id").cloned() else {
                                continue;
                            };
                            let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                            if method == "textDocument/hover" && attempt == 0 {
                                // The first connection dies mid-request, as if the gateway had
                                // restarted: no response, socket closed.
                                return;
                            }
                            let result = serde_json::json!({ "contents": { "kind": "markdown", "value": "ok" } });
                            let response =
                                serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result });
                            let _ = framed
                                .send(WireMessage::LspPayload(response.to_string()))
                                .await;
                        }
                        WireMessage::Disconnect { .. } => return,
                        _ => {}
                    }
                }
            });
        }
    });

    let params = serde_json::json!({ "textDocument": { "uri": format!("file://{}", file.display()) } });
    let result = prod_code_mcp::session::pooled_query(addr, &root, &file, "textDocument/hover", params)
        .await
        .expect("the retry after a dead connection succeeds");
    assert_eq!(result["contents"]["value"], "ok");
    assert_eq!(
        attempts.load(Ordering::SeqCst),
        2,
        "the dead connection is replaced by a fresh one"
    );
}
