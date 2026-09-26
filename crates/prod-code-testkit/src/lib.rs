//! A gateway that answers from a script, and workspaces to point it at.
//!
//! The tools this repository ships are mostly compositions of analyzer answers: ask for a
//! symbol, ask for a rename, merge what comes back, check the result. The bugs live in the
//! composition, not in the analyzers, so testing them does not need rust-analyzer, gopls or a
//! LAN node — it needs answers of the right shape, on demand, in the right order.
//!
//! [`ScriptedGateway`] is a real TCP server speaking the real wire protocol: it accepts the
//! pre-flight sync, answers the handshake, replies to `initialize` itself, and hands every
//! other LSP request to a closure the test provides. [`Workspace`] is the checkout the client
//! syncs — a temporary directory with a commit in it, because the pre-flight sync measures a
//! delta against `HEAD`.
//!
//! ```no_run
//! # use prod_code_testkit::{ScriptedGateway, Workspace, answers};
//! # async fn example() {
//! let workspace = Workspace::new(&[("src/lib.rs", "pub fn a() {}\n")]);
//! let gateway = ScriptedGateway::start(|method, _params| match method {
//!     "textDocument/diagnostic" => answers::no_diagnostics(),
//!     _ => serde_json::Value::Null,
//! })
//! .await;
//! // … drive a tool at `gateway.addr()` against `workspace.root()` …
//! # }
//! ```

use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, ReadFileResponse, SyncProbeResponse,
    SyncResponse, WireMessage,
};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

/// Answers one LSP request: the JSON-RPC method and its params, in, the `result` out.
pub type Answer = Arc<dyn Fn(&str, &serde_json::Value) -> serde_json::Value + Send + Sync>;

/// A gateway that syncs nothing, loads nothing, and answers from a script.
pub struct ScriptedGateway {
    addr: SocketAddr,
    calls: Arc<AtomicUsize>,
}

impl ScriptedGateway {
    /// Starts one on an ephemeral port. It lives as long as the test process.
    pub async fn start<F>(answer: F) -> Self
    where
        F: Fn(&str, &serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    {
        Self::start_arc(Arc::new(answer)).await
    }

    /// Starts one from an already shared closure, for a script that keeps state of its own.
    pub async fn start_arc(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let answer = Arc::clone(&answer);
                let counted = Arc::clone(&counted);
                tokio::spawn(async move {
                    let _ = serve(socket, answer, counted).await;
                });
            }
        });
        Self { addr, calls }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// How many LSP requests the script has been asked, `initialize` aside. A tool that asks
    /// the analyzer twice about the same place is a bug this number catches.
    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

async fn serve(socket: TcpStream, answer: Answer, calls: Arc<AtomicUsize>) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(message) = framed.next().await {
        match message? {
            WireMessage::SyncProbeRequest(req) => {
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root.clone(),
                        seeded: false,
                        files_deleted: 0,
                        // Nothing is missing: the gateway pretends it already holds the tree,
                        // so a test never depends on what the client decides to upload.
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
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(req) => {
                // Shown to the script as `prod-code/handshake`, so a test can see which engine
                // a session asked for (`purpose`); an answer with `engine_age_ms` says how long
                // ago the engine was loaded (#381), any other answer leaves it unsaid.
                let said = answer(
                    "prod-code/handshake",
                    &serde_json::json!({ "purpose": req.purpose.clone() }),
                );
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: std::process::id(),
                        session_id: 1,
                        // The same path on both sides, so `PathTranslator` is the identity and
                        // a script can answer with the test's own paths.
                        server_workspace_root: req.client_workspace_root.clone(),
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                        engine_age_ms: said.get("engine_age_ms").and_then(|v| v.as_u64()),
                        index_gated: said
                            .get("index_gated")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                    }))
                    .await?;
            }
            // A file that lives only on the node (a dependency's source) is read through the
            // script as the pseudo-method `prod-code/readFile`: a string is the file's text,
            // anything else means it cannot be read.
            WireMessage::ReadFileRequest(req) => {
                let text = answer(
                    "prod-code/readFile",
                    &serde_json::json!({ "path": req.path.clone() }),
                );
                let (content, error) = match text.as_str() {
                    Some(text) => (Some(text.as_bytes().to_vec()), None),
                    None => (None, Some(format!("no such file: {}", req.path))),
                };
                framed
                    .send(WireMessage::ReadFileResponse(ReadFileResponse {
                        path: req.path,
                        content,
                        truncated: false,
                        error,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(json) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                    continue;
                };
                let Some(id) = value.get("id").cloned() else {
                    // A notification — didOpen, didChange, initialized — is shown to the script,
                    // which may keep the text it carries, and gets no answer.
                    if let Some(method) = value.get("method").and_then(|m| m.as_str()) {
                        let params = value
                            .get("params")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
                        let _ = answer(method, &params);
                    }
                    continue;
                };
                let method = value.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let params = value
                    .get("params")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                let result = if method == "initialize" {
                    serde_json::json!({ "capabilities": { "hoverProvider": true } })
                } else {
                    calls.fetch_add(1, Ordering::Relaxed);
                    answer(method, &params)
                };
                // A request of the server's own that the script wants passed on before the
                // answer (`prod-code/server-request` returns it; it is given the question's id),
                // as an older gateway passed on gopls's (#391).
                if prod_code_protocol::readiness::needs_index(method) {
                    let mut request = answer(
                        "prod-code/server-request",
                        &serde_json::json!({ "method": method }),
                    );
                    if request.get("method").is_some() {
                        request["id"] = id.clone();
                        framed
                            .send(WireMessage::LspPayload(request.to_string()))
                            .await?;
                    }
                }
                // An index question the script says the server answered while still indexing
                // (`prod-code/busy` returns the work) comes with the gateway's note (#391).
                if prod_code_protocol::readiness::needs_index(method) {
                    let busy = answer("prod-code/busy", &serde_json::json!({ "method": method }));
                    if busy.get("title").is_some() {
                        let note = serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": prod_code_protocol::readiness::BUSY_NOTIFICATION,
                            "params": busy
                        });
                        framed
                            .send(WireMessage::LspPayload(note.to_string()))
                            .await?;
                    }
                }
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

/// A checkout for the client to sync: files, and a commit so the pre-flight sync has a base.
pub struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    /// An empty repository: write the files, then [`Workspace::commit`] them.
    pub fn empty() -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("tempdir"),
        };
        workspace.commit();
        workspace
    }

    /// Writes the files, makes it a repository and commits them.
    pub fn new(files: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let workspace = Self { dir };
        for (rel, text) in files {
            workspace.write(rel, text);
        }
        workspace.commit();
        workspace
    }

    /// The canonical root. Canonical because the client canonicalises it too, and on macOS a
    /// temporary directory is reached through a symlink.
    pub fn root(&self) -> PathBuf {
        std::fs::canonicalize(self.dir.path()).unwrap_or_else(|_| self.dir.path().to_path_buf())
    }

    pub fn path(&self, rel: &str) -> PathBuf {
        self.root().join(rel)
    }

    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.path(rel)).expect("read")
    }

    /// Writes one file, creating its directories. Not committed: call [`Workspace::commit`].
    pub fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(&path, text).expect("write");
        path
    }

    /// `git init` on first use, then add and commit everything.
    pub fn commit(&self) {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(self.dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git runs")
        };
        if !self.dir.path().join(".git").is_dir() {
            assert!(git(&["init", "-q"]).success(), "git init");
        }
        assert!(git(&["add", "-A"]).success(), "git add");
        git(&[
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=test",
            "commit",
            "-qm",
            "fixture",
        ]);
    }
}

/// The answer shapes the engines really return, so a script does not have to spell them out.
pub mod answers {
    use super::*;

    /// A clean pull-diagnostics report.
    pub fn no_diagnostics() -> serde_json::Value {
        serde_json::json!({ "kind": "full", "items": [] })
    }

    /// A pull-diagnostics report with one error at a 1-based line and column.
    pub fn error_at(line: u32, col: u32, code: &str, message: &str) -> serde_json::Value {
        serde_json::json!({ "kind": "full", "items": [ {
            "severity": 1,
            "code": code,
            "message": message,
            "range": {
                "start": { "line": line - 1, "character": col - 1 },
                "end": { "line": line - 1, "character": col }
            }
        } ] })
    }

    /// A workspace edit that replaces a file wholesale — how the in-process Rust engine
    /// answers a rename or a structural rewrite.
    pub fn whole_file(path: &Path, old: &str, new_text: &str) -> serde_json::Value {
        serde_json::json!({ "documentChanges": [ {
            "textDocument": { "uri": uri(path), "version": null },
            "edits": [ {
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": old.lines().count(), "character": 0 }
                },
                "newText": new_text
            } ]
        } ] })
    }

    /// A workspace edit with one edit per occurrence — how gopls and the TypeScript server
    /// answer a rename. Each spot is (line, column, length, replacement), 1-based.
    pub fn ranged(path: &Path, spots: &[(u32, u32, usize, &str)]) -> serde_json::Value {
        let edits: Vec<serde_json::Value> = spots
            .iter()
            .map(|(line, col, len, text)| {
                serde_json::json!({
                    "range": {
                        "start": { "line": line - 1, "character": col - 1 },
                        "end": { "line": line - 1, "character": col - 1 + *len as u32 }
                    },
                    "newText": text
                })
            })
            .collect();
        serde_json::json!({ "documentChanges": [ {
            "textDocument": { "uri": uri(path), "version": null },
            "edits": edits
        } ] })
    }

    /// Locations, as `textDocument/references` and `definition` return them. Each spot is
    /// (line, column), 1-based.
    pub fn locations(path: &Path, spots: &[(u32, u32)]) -> serde_json::Value {
        serde_json::Value::Array(
            spots
                .iter()
                .map(|(line, col)| {
                    serde_json::json!({
                        "uri": uri(path),
                        "range": {
                            "start": { "line": line - 1, "character": col - 1 },
                            "end": { "line": line - 1, "character": col }
                        }
                    })
                })
                .collect(),
        )
    }

    /// One `workspace/symbol` hit. `kind` is the LSP symbol kind (5 class, 12 function,
    /// 23 struct).
    pub fn symbol(name: &str, kind: u32, path: &Path, line: u32, col: u32) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "kind": kind,
            "location": {
                "uri": uri(path),
                "range": {
                    "start": { "line": line - 1, "character": col - 1 },
                    "end": { "line": line - 1, "character": col - 1 + name.chars().count() as u32 }
                }
            }
        })
    }

    /// One `textDocument/documentSymbol` node, covering lines `from`..=`to` (1-based) with its
    /// name at `col` on `from`.
    pub fn document_symbol(
        name: &str,
        kind: u32,
        from: u32,
        to: u32,
        col: u32,
    ) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "kind": kind,
            "range": {
                "start": { "line": from - 1, "character": 0 },
                "end": { "line": to - 1, "character": 1 }
            },
            "selectionRange": {
                "start": { "line": from - 1, "character": col - 1 },
                "end": { "line": from - 1, "character": col - 1 + name.chars().count() as u32 }
            }
        })
    }

    /// A `textDocument/documentSymbol` node with the ones nested in it, the way clangd and
    /// sourcekit-lsp answer: a namespace holding a class holding its methods.
    pub fn nested(
        symbol: serde_json::Value,
        children: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let mut symbol = symbol;
        symbol["children"] = serde_json::Value::Array(children);
        symbol
    }

    /// Locations of zero width, as sourcekit-lsp answers `textDocument/references`: a range
    /// that starts and ends where the name starts. Each spot is (line, column), 1-based.
    pub fn points(path: &Path, spots: &[(u32, u32)]) -> serde_json::Value {
        serde_json::Value::Array(
            spots
                .iter()
                .map(|(line, col)| {
                    let at = serde_json::json!({ "line": line - 1, "character": col - 1 });
                    serde_json::json!({ "uri": uri(path), "range": { "start": at, "end": at } })
                })
                .collect(),
        )
    }

    /// Markdown hover contents, the shape every engine answers with.
    pub fn hover(markdown: &str) -> serde_json::Value {
        serde_json::json!({ "contents": { "kind": "markdown", "value": markdown } })
    }

    /// The `file://` URI of `path`, as the other answers spell it.
    pub fn uri(path: &Path) -> String {
        format!("file://{}", path.display())
    }
}
