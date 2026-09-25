//! `execute_tool` end to end: every tool name is driven against a scripted gateway and the
//! rendered text is asserted, not just "it returned something". `list_tools()` itself is
//! covered by the unit tests inside `src/tools.rs`; this file is the dispatch table.
//!
//! Most tools speak only the LSP protocol and use `ScriptedGateway` from the shared harness.
//! `code_exec`, `code_check`/`code_lint`/`code_test`, `code_diagnose_failure`, `code_search`,
//! `code_shadow_run`, `code_status` and `code_source` speak other wire messages (`ExecRequest`,
//! `SearchRequest`, `ShadowRunRequest`, `StatusRequest`, `ReadFileRequest`); `mock_gateway`
//! below answers those too, alongside the same LSP script.

use futures_util::{SinkExt, StreamExt};
use prod_code_mcp::protocol::McpContentItem;
use prod_code_mcp::tools::execute_tool;
use prod_code_protocol::{
    ExecChunk, ExecExit, HandshakeResponse, PROTOCOL_VERSION, ProdCodeCodec, ReadFileResponse,
    SearchHit, SearchResponse, ShadowHypothesisResult, ShadowRunResponse, StatusResponse,
    SyncProbeResponse, SyncResponse, WireMessage,
};
use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::Framed;

fn workspace() -> Workspace {
    Workspace::empty()
}

fn write(ws: &Workspace, rel: &str, text: &str) -> PathBuf {
    ws.write(rel, text)
}

fn commit(ws: &Workspace) {
    ws.commit();
}

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

fn text_of(result: &prod_code_mcp::protocol::McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|c| match c {
            McpContentItem::Text { text } => text.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The rust fixture most tools need: a package and one small function.
fn rust_workspace(source: &str) -> Workspace {
    Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", source),
    ])
}

/// What `code_exec`, `code_check`/`code_lint`/`code_test`, `code_search`, `code_shadow_run`,
/// `code_status` and `code_source` need beyond the LSP protocol.
#[derive(Clone)]
struct Script {
    lsp: Answer,
    exec_stdout: Vec<u8>,
    exec_stderr: Vec<u8>,
    exec_exit: Option<i32>,
    /// Files the command rewrote, sent back as the gateway does for a formatter.
    exec_changes: Vec<prod_code_protocol::FileDelta>,
    /// Sends `exec_changes` only for a command with this argument, the way a linter rewrites
    /// files only in its fix mode. The client writes back only what differs from the checkout,
    /// so changes sent for every command would all land with the first one (#254).
    exec_changes_only_for: Option<&'static str>,
    /// What the command used, as the gateway reports it from `wait4`.
    exec_usage: Option<prod_code_protocol::ExecUsage>,
    /// The node's platform, as the gateway reports it (#140).
    exec_platform: Option<String>,
    /// The environment of every command the mock was asked to run.
    exec_env: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    search_hits: Vec<SearchHit>,
    shadow_results: Vec<ShadowHypothesisResult>,
    read_file: Option<Vec<u8>>,
}

impl Default for Script {
    fn default() -> Self {
        Self {
            lsp: Arc::new(|_, _| serde_json::Value::Null),
            exec_stdout: Vec::new(),
            exec_stderr: Vec::new(),
            exec_exit: Some(0),
            exec_changes: Vec::new(),
            exec_changes_only_for: None,
            exec_usage: None,
            exec_platform: None,
            exec_env: Arc::default(),
            search_hits: Vec::new(),
            shadow_results: Vec::new(),
            read_file: Some(b"stub".to_vec()),
        }
    }
}

async fn mock_gateway(script: Script) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            let script = script.clone();
            tokio::spawn(async move {
                let _ = serve_mock(socket, script).await;
            });
        }
    });
    addr
}

async fn serve_mock(socket: TcpStream, script: Script) -> anyhow::Result<()> {
    let mut framed = Framed::new(socket, ProdCodeCodec::new());
    while let Some(msg) = framed.next().await {
        match msg? {
            WireMessage::SyncProbeRequest(req) => {
                framed
                    .send(WireMessage::SyncProbeResponse(SyncProbeResponse {
                        server_workspace_root: req.client_workspace_root,
                        seeded: false,
                        files_deleted: 0,
                        missing: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::SyncRequest(req) => {
                framed
                    .send(WireMessage::SyncResponse(SyncResponse {
                        server_workspace_root: req.client_workspace_root,
                        files_updated: 0,
                        files_deleted: 0,
                        bytes_transferred: 0,
                        duration_ms: 1,
                        workspace_was_fresh: false,
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::HandshakeRequest(req) => {
                framed
                    .send(WireMessage::HandshakeResponse(HandshakeResponse {
                        protocol_version: PROTOCOL_VERSION,
                        server_pid: std::process::id(),
                        session_id: 1,
                        server_workspace_root: req.client_workspace_root,
                        detected_engine: "rust".to_string(),
                        stale_paths: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::StatusRequest => {
                framed
                    .send(WireMessage::StatusResponse(StatusResponse {
                        server_pid: std::process::id(),
                        uptime_seconds: 7265,
                        active_sessions: 2,
                        loaded_workspaces: 1,
                        detected_engines: vec!["rust".to_string()],
                        memory_rss_bytes: Some(64 * 1024 * 1024),
                        total_queries: 42,
                        active_queries: 0,
                        load_average_millis: Some(200),
                        cpu_count: Some(4),
                        platform: None,
                        running_commands: Vec::new(),
                    }))
                    .await?;
            }
            WireMessage::ReadFileRequest(req) => {
                framed
                    .send(WireMessage::ReadFileResponse(ReadFileResponse {
                        path: req.path,
                        content: script.read_file.clone(),
                        truncated: false,
                        error: if script.read_file.is_some() {
                            None
                        } else {
                            Some("no such file".to_string())
                        },
                    }))
                    .await?;
            }
            WireMessage::ExecRequest(req) => {
                script
                    .exec_env
                    .lock()
                    .unwrap()
                    .extend(req.env.iter().cloned());
                // In small pieces, as a long run's output arrives: a line can be split anywhere.
                for piece in script.exec_stdout.chunks(7) {
                    framed
                        .send(WireMessage::ExecChunk(ExecChunk {
                            stderr: false,
                            data: Some(piece.to_vec()),
                        }))
                        .await?;
                }
                if !script.exec_stderr.is_empty() {
                    framed
                        .send(WireMessage::ExecChunk(ExecChunk {
                            stderr: true,
                            data: Some(script.exec_stderr.clone()),
                        }))
                        .await?;
                }
                if !script.exec_changes.is_empty()
                    && script
                        .exec_changes_only_for
                        .is_none_or(|arg| req.command.iter().any(|a| a == arg))
                {
                    framed
                        .send(WireMessage::ExecChanges(prod_code_protocol::ExecChanges {
                            files: script.exec_changes.clone(),
                        }))
                        .await?;
                }
                framed
                    .send(WireMessage::ExecExit(ExecExit {
                        exit_code: script.exec_exit,
                        duration_ms: 5,
                        server_workspace_root: req.client_workspace_root,
                        timed_out: false,
                        error: None,
                        usage: script.exec_usage,
                        platform: script.exec_platform.clone(),
                    }))
                    .await?;
            }
            WireMessage::SearchRequest(req) => {
                framed
                    .send(WireMessage::SearchResponse(SearchResponse {
                        server_workspace_root: req.client_workspace_root,
                        hits: script.search_hits.clone(),
                        indexed_files: 3,
                        indexed_declarations: script.search_hits.len(),
                        took_ms: 2,
                        error: None,
                        dense: None,
                    }))
                    .await?;
            }
            WireMessage::ShadowRunRequest(req) => {
                framed
                    .send(WireMessage::ShadowRunResponse(ShadowRunResponse {
                        server_workspace_root: req.client_workspace_root,
                        mode: "in-place".to_string(),
                        results: script.shadow_results.clone(),
                        error: None,
                    }))
                    .await?;
            }
            WireMessage::LspPayload(json) => {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) else {
                    continue;
                };
                let Some(id) = value.get("id").cloned() else {
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
                    (script.lsp)(method, &params)
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

// ---------------------------------------------------------------------------------------
// Argument validation: every one of these returns before any network call, so a dummy,
// unreachable remote address is fine.
// ---------------------------------------------------------------------------------------

fn nowhere() -> SocketAddr {
    "127.0.0.1:1".parse().unwrap()
}

#[tokio::test]
async fn unknown_tool_name_is_reported_by_name() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_frobnicate",
        serde_json::json!({}),
    )
    .await
    .expect("dispatch does not fail, it answers with an error result");
    assert!(err.is_error);
    assert_eq!(text_of(&err), "Unknown tool: code_frobnicate");
}

#[tokio::test]
async fn position_tools_require_path_line_and_character() {
    let ws = workspace();
    for tool in [
        "code_definition",
        "code_references",
        "code_callers",
        "code_callees",
        "code_implementations",
        "code_hover",
        "code_type_at",
        "code_safe_delete",
    ] {
        let err = execute_tool(nowhere(), &ws.root(), tool, serde_json::json!({}))
            .await
            .expect_err(&format!("{tool} needs path/line/character"));
        assert!(
            format!("{err:#}").contains("Missing 'path' argument"),
            "{tool}: {err:#}"
        );
        let err = execute_tool(
            nowhere(),
            &ws.root(),
            tool,
            serde_json::json!({ "path": "src/lib.rs" }),
        )
        .await
        .expect_err(&format!("{tool} needs line"));
        assert!(
            format!("{err:#}").contains("Missing 'line' argument"),
            "{tool}: {err:#}"
        );
        let err = execute_tool(
            nowhere(),
            &ws.root(),
            tool,
            serde_json::json!({ "path": "src/lib.rs", "line": 1 }),
        )
        .await
        .expect_err(&format!("{tool} needs character"));
        assert!(
            format!("{err:#}").contains("Missing 'character' argument"),
            "{tool}: {err:#}"
        );
    }
}

#[tokio::test]
async fn code_rename_requires_a_new_name() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_rename",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 1 }),
    )
    .await
    .expect_err("new_name is required");
    assert!(format!("{err:#}").contains("Missing 'new_name' argument"));
}

#[tokio::test]
async fn code_assist_requires_an_id() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_assist",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 1 }),
    )
    .await
    .expect_err("id is required");
    assert!(format!("{err:#}").contains("Missing 'id' argument"));
}

#[tokio::test]
async fn code_schema_rename_requires_field_and_to() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_schema_rename",
        serde_json::json!({}),
    )
    .await
    .expect_err("field is required");
    assert!(format!("{err:#}").contains("Missing 'field' argument"));
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_schema_rename",
        serde_json::json!({ "field": "order_id" }),
    )
    .await
    .expect_err("to is required");
    assert!(format!("{err:#}").contains("Missing 'to' argument"));

    // `repos` is a list of readable paths, and it does not mix with `path` or `verify`.
    for (repos, extra, expected) in [
        (serde_json::json!("../frontend"), "", "a list of paths"),
        (serde_json::json!([1]), "", "repos takes paths"),
        (serde_json::json!(["no-such-repo"]), "", "cannot be read"),
        (serde_json::json!(["."]), "path", "drop them"),
        (serde_json::json!(["."]), "verify", "drop them"),
    ] {
        let mut args = serde_json::json!({ "field": "order_id", "to": "trade_id", "repos": repos });
        match extra {
            "path" => args["path"] = serde_json::json!("src"),
            "verify" => args["verify"] = serde_json::json!("compile"),
            _ => {}
        }
        let err = execute_tool(nowhere(), &ws.root(), "code_schema_rename", args)
            .await
            .expect_err("a bad `repos` is refused");
        assert!(format!("{err:#}").contains(expected), "{expected}: {err:#}");
    }
}

#[tokio::test]
async fn code_change_signature_requires_params() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_change_signature",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 1 }),
    )
    .await
    .expect_err("params is required");
    assert!(format!("{err:#}").contains("Missing 'params' argument"));
}

#[tokio::test]
async fn code_generate_fixture_requires_a_symbol() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_generate_fixture",
        serde_json::json!({}),
    )
    .await
    .expect_err("symbol is required");
    assert!(format!("{err:#}").contains("Missing 'symbol' argument"));
}

#[tokio::test]
async fn code_search_requires_a_query() {
    let ws = workspace();
    let err = execute_tool(nowhere(), &ws.root(), "code_search", serde_json::json!({}))
        .await
        .expect_err("query is required");
    assert!(format!("{err:#}").contains("Missing 'query' argument"));
}

#[tokio::test]
async fn code_source_requires_a_path() {
    let ws = workspace();
    let err = execute_tool(nowhere(), &ws.root(), "code_source", serde_json::json!({}))
        .await
        .expect_err("path is required");
    assert!(format!("{err:#}").contains("Missing 'path' argument"));
}

#[tokio::test]
async fn code_exec_requires_argv() {
    let ws = workspace();
    let err = execute_tool(nowhere(), &ws.root(), "code_exec", serde_json::json!({}))
        .await
        .expect_err("argv is required");
    assert!(format!("{err:#}").contains("Missing 'argv' argument"));
}

#[tokio::test]
async fn code_slice_requires_path_and_line() {
    let ws = workspace();
    let err = execute_tool(nowhere(), &ws.root(), "code_slice", serde_json::json!({}))
        .await
        .expect_err("path is required");
    assert!(format!("{err:#}").contains("Missing 'path' argument"));
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_slice",
        serde_json::json!({ "path": "src/lib.rs" }),
    )
    .await
    .expect_err("line is required");
    assert!(format!("{err:#}").contains("Missing 'line' argument"));
}

#[tokio::test]
async fn code_diagnostics_and_validate_edit_require_a_path_and_new_text() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_diagnostics",
        serde_json::json!({}),
    )
    .await
    .expect_err("path is required");
    assert!(format!("{err:#}").contains("Missing 'path' argument"));

    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_validate_edit",
        serde_json::json!({}),
    )
    .await
    .expect_err("path is required");
    assert!(format!("{err:#}").contains("Missing 'path' argument"));

    let result = execute_tool(
        nowhere(),
        &ws.root(),
        "code_validate_edit",
        serde_json::json!({ "path": "src/lib.rs" }),
    )
    .await
    .expect("a missing new_text is reported as a tool error, not a Rust error");
    assert!(result.is_error);
    assert_eq!(text_of(&result), "Missing 'new_text' argument");
}

#[tokio::test]
async fn code_validate_edits_requires_a_non_empty_edits_list_with_path_and_new_text() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({}),
    )
    .await
    .expect_err("edits is required");
    assert!(format!("{err:#}").contains("Missing 'edits' argument"));

    let result = execute_tool(
        nowhere(),
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({ "edits": [] }),
    )
    .await
    .expect("an empty list is a tool error");
    assert!(result.is_error);
    assert_eq!(
        text_of(&result),
        "the change touches no file that can be checked"
    );

    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({ "edits": [ { "new_text": "x" } ] }),
    )
    .await
    .expect_err("an edit without a path is refused");
    assert!(format!("{err:#}").contains("edit without 'path'"));

    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({ "edits": [ { "path": "src/lib.rs" } ] }),
    )
    .await
    .expect_err("an edit without new_text is refused");
    assert!(format!("{err:#}").contains("without 'new_text'"));
}

#[tokio::test]
async fn code_shadow_run_requires_argv_and_hypotheses() {
    let ws = workspace();
    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_shadow_run",
        serde_json::json!({}),
    )
    .await
    .expect_err("argv is required");
    assert!(format!("{err:#}").contains("Missing 'argv' argument"));

    let result = execute_tool(
        nowhere(),
        &ws.root(),
        "code_shadow_run",
        serde_json::json!({ "argv": [] }),
    )
    .await
    .expect("an empty argv is a tool error");
    assert!(result.is_error);
    assert_eq!(text_of(&result), "'argv' is empty");

    let err = execute_tool(
        nowhere(),
        &ws.root(),
        "code_shadow_run",
        serde_json::json!({ "argv": ["cargo", "test"] }),
    )
    .await
    .expect_err("hypotheses is required");
    assert!(format!("{err:#}").contains("missing 'hypotheses' array"));
}

#[tokio::test]
async fn code_codemod_requires_a_rule_shaped_like_pattern_to_replacement() {
    let ws = workspace();
    let err = execute_tool(nowhere(), &ws.root(), "code_codemod", serde_json::json!({}))
        .await
        .expect_err("rule is required");
    assert!(format!("{err:#}").contains("Missing 'rule' argument"));

    let result = execute_tool(
        nowhere(),
        &ws.root(),
        "code_codemod",
        serde_json::json!({ "rule": "$a.unwrap()" }),
    )
    .await
    .expect("a rule without ==>> is a tool error, not a network call");
    assert!(result.is_error);
    assert!(text_of(&result).contains("pattern ==>> replacement"));
}

// ---------------------------------------------------------------------------------------
// Read-only navigation tools, against a plain LSP-only ScriptedGateway.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn code_symbols_lists_hits_and_reports_when_there_are_none() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn record() {}\n");
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "workspace/symbol" => {
            serde_json::Value::Array(vec![answers::symbol("record", 12, &path, 1, 8)])
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_symbols",
        serde_json::json!({ "query": "record" }),
    )
    .await
    .expect("the search runs");
    assert!(!result.is_error);
    let text = text_of(&result);
    assert!(text.contains("1 symbol(s) matching `record`"), "{text}");
    assert!(text.contains("[Function] record"), "{text}");
}

#[tokio::test]
async fn code_definition_reports_locations_or_says_there_are_none() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn a() {}\n");
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/definition" => answers::locations(&path, &[(3, 5)]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("the query runs");
    assert!(
        text_of(&result).contains("Definition:"),
        "{}",
        text_of(&result)
    );
    assert!(text_of(&result).contains(":3:5"), "{}", text_of(&result));

    let empty_remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/definition" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let none = execute_tool(
        empty_remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("the query runs");
    assert_eq!(text_of(&none), "No definition found.");
}

#[tokio::test]
async fn code_references_counts_hits_and_says_when_there_are_none() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn a() {}\n");
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/references" => answers::locations(&path, &[(2, 1), (5, 3)]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_references",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("the query runs");
    assert!(
        text_of(&result).contains("Found 2 reference(s)"),
        "{}",
        text_of(&result)
    );

    let empty_remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let none = execute_tool(
        empty_remote,
        &ws.root(),
        "code_references",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("the query runs");
    assert_eq!(text_of(&none), "No references found.");
}

#[tokio::test]
async fn code_callers_and_callees_walk_the_call_hierarchy() {
    let ws = workspace();
    let lib = write(
        &ws,
        "src/lib.rs",
        "pub fn callee() {}\n\npub fn caller() {\n    callee();\n}\n",
    );
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/prepareCallHierarchy" => serde_json::json!([
            { "name": "callee", "uri": format!("file://{}", path.display()),
              "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 13 } } }
        ]),
        "callHierarchy/incomingCalls" => serde_json::json!([
            { "from": { "name": "caller", "uri": format!("file://{}", path.display()),
                        "selectionRange": { "start": { "line": 2, "character": 7 }, "end": { "line": 2, "character": 13 } } },
              "fromRanges": [ { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 10 } } ] }
        ]),
        "callHierarchy/outgoingCalls" => serde_json::json!([]),
        _ => {
            let _ = params;
            serde_json::Value::Null
        }
    }))
    .await;
    let callers = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("callers run");
    let text = text_of(&callers);
    assert!(text.contains("`callee`: 1 caller(s)"), "{text}");
    assert!(text.contains("caller"), "{text}");

    let callees = execute_tool(
        remote,
        &ws.root(),
        "code_callees",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("callees run");
    assert!(
        text_of(&callees).contains("`callee`: 0 callee(s)"),
        "{}",
        text_of(&callees)
    );
}

/// `depth` walks the callers of the callers. `mid` calls `leaf`, `top` and `leaf` call `mid`:
/// the tree reaches `top`, and `leaf` appears again under `mid`, marked, not expanded a second
/// time.
#[tokio::test]
async fn code_callers_to_a_depth_is_a_tree_that_ends_at_recursion() {
    let ws = workspace();
    let lib = write(
        &ws,
        "src/lib.rs",
        "pub fn leaf() {\n    mid();\n}\n\npub fn mid() {\n    leaf();\n}\n\npub fn top() {\n    mid();\n}\n",
    );
    commit(&ws);
    let uri = format!("file://{}", lib.display());
    let item = |name: &str, line: u32| {
        serde_json::json!({ "name": name, "uri": uri.clone(),
            "selectionRange": { "start": { "line": line, "character": 7 }, "end": { "line": line, "character": 10 } } })
    };
    let edge = |name: &str, line: u32, call: u32| {
        serde_json::json!({ "from": item(name, line),
            "fromRanges": [ { "start": { "line": call, "character": 4 }, "end": { "line": call, "character": 7 } } ] })
    };
    let answers = (
        serde_json::json!([item("leaf", 0)]),
        serde_json::json!([edge("mid", 4, 5)]),
        serde_json::json!([edge("leaf", 0, 1), edge("top", 8, 9)]),
    );
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/prepareCallHierarchy" => answers.0.clone(),
        "callHierarchy/incomingCalls" => {
            match params.pointer("/item/name").and_then(|n| n.as_str()) {
                Some("leaf") => answers.1.clone(),
                Some("mid") => answers.2.clone(),
                _ => serde_json::json!([]),
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let at = |depth: u64| serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8, "depth": depth });
    let tree = text_of(
        &execute_tool(remote, &ws.root(), "code_callers", at(3))
            .await
            .expect("callers run"),
    );
    let u = format!("file://{}", lib.display());
    assert_eq!(
        tree,
        format!(
            "`leaf`: 1 caller(s), 3 in all to depth 3\n  \
             • mid  {u}:5:8  [call sites: 6:5]\n    \
             • leaf  {u}:1:8  [call sites: 2:5]  (shown above)\n    \
             • top  {u}:9:8  [call sites: 10:5]"
        )
    );
    // Depth 1 is the direct callers only, as before.
    let direct = text_of(
        &execute_tool(remote, &ws.root(), "code_callers", at(1))
            .await
            .expect("callers run"),
    );
    assert_eq!(
        direct,
        format!("`leaf`: 1 caller(s)\n  • mid  {u}:5:8  [call sites: 6:5]")
    );
}

/// A Rust type's supertypes: the derives from its attributes (a built-in one and a macro's),
/// and the trait of a written impl; the inherent impl is not one. A trait's are its bounds.
#[tokio::test]
async fn code_supertypes_reads_derives_impls_and_supertraits() {
    let ws = workspace();
    let lib = write(
        &ws,
        "src/lib.rs",
        "#[derive(Clone, serde::Serialize)]\npub struct Cache;\n\nimpl Default for Cache {\n    fn default() -> Self { Cache }\n}\n\nimpl Cache {}\n\npub trait Store: Send + Sync {}\n",
    );
    commit(&ws);
    let uri = format!("file://{}", lib.display());
    let at = |line: u32, character: u32| {
        serde_json::json!({ "uri": uri.clone(), "range": {
            "start": { "line": line, "character": character }, "end": { "line": line, "character": character + 5 } } })
    };
    let answers = (
        at(1, 11),
        serde_json::json!([at(0, 9), at(1, 11), at(3, 17), at(7, 5)]),
    );
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let line = params.pointer("/position/line").and_then(|l| l.as_u64());
        match (method, line) {
            ("textDocument/definition", Some(1)) => answers.0.clone(),
            ("textDocument/definition", Some(9)) => serde_json::json!([]),
            ("textDocument/implementation", Some(1)) => answers.1.clone(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let cache = text_of(
        &execute_tool(
            remote,
            &ws.root(),
            "code_supertypes",
            serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 12 }),
        )
        .await
        .expect("supertypes run"),
    );
    assert_eq!(
        cache,
        "`Cache` implements 3 trait(s):\n  • Clone  (derived)  src/lib.rs:1:10\n  • Default  src/lib.rs:4:18\n  • serde::Serialize  (derived)  src/lib.rs:1:17"
    );
    let store = text_of(
        &execute_tool(
            remote,
            &ws.root(),
            "code_supertypes",
            serde_json::json!({ "path": "src/lib.rs", "line": 10, "character": 11 }),
        )
        .await
        .expect("supertypes run"),
    );
    assert_eq!(
        store,
        "`Store` requires 2 supertrait(s):\n  • Send\n  • Sync"
    );
}

/// Another language's server is asked for its type hierarchy, and one without it is named.
#[tokio::test]
async fn code_supertypes_asks_other_servers_for_their_type_hierarchy() {
    let ws = workspace();
    let go = write(&ws, "shape.go", "package shape\n\ntype Square struct{}\n");
    commit(&ws);
    let uri = format!("file://{}", go.display());
    let item = serde_json::json!({ "name": "Square", "kind": 23, "uri": uri.clone(),
        "range": { "start": { "line": 2, "character": 5 }, "end": { "line": 2, "character": 11 } },
        "selectionRange": { "start": { "line": 2, "character": 5 }, "end": { "line": 2, "character": 11 } } });
    let shape = serde_json::json!([{ "name": "Shape", "kind": 11, "uri": uri.clone(),
        "range": { "start": { "line": 9, "character": 5 }, "end": { "line": 9, "character": 10 } },
        "selectionRange": { "start": { "line": 9, "character": 5 }, "end": { "line": 9, "character": 10 } } }]);
    let remote = scripted_gateway(Arc::new(move |method, params| {
        match (
            method,
            params.pointer("/position/line").and_then(|l| l.as_u64()),
        ) {
            ("textDocument/prepareTypeHierarchy", Some(2)) => serde_json::json!([item.clone()]),
            ("textDocument/prepareTypeHierarchy", _) => serde_json::Value::Null,
            ("typeHierarchy/supertypes", _) => shape.clone(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let found = text_of(
        &execute_tool(
            remote,
            &ws.root(),
            "code_supertypes",
            serde_json::json!({ "path": "shape.go", "line": 3, "character": 6 }),
        )
        .await
        .expect("supertypes run"),
    );
    assert_eq!(
        found,
        "`Square` has 1 supertype(s):\n  • Shape  shape.go:10:6"
    );
    let none = text_of(
        &execute_tool(
            remote,
            &ws.root(),
            "code_supertypes",
            serde_json::json!({ "path": "shape.go", "line": 1, "character": 1 }),
        )
        .await
        .expect("supertypes run"),
    );
    assert!(none.starts_with("No type hierarchy at"), "{none}");
}

#[tokio::test]
async fn code_callers_reports_no_function_at_position() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "// comment\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/prepareCallHierarchy" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_callers",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 1 }),
    )
    .await
    .expect("the query runs");
    assert!(
        text_of(&result).contains("No function at"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn code_implementations_lists_locations_or_says_there_are_none() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub trait Shape {}\n");
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/implementation" => answers::locations(&path, &[(4, 1)]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_implementations",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 11 }),
    )
    .await
    .expect("the query runs");
    assert!(
        text_of(&result).contains("Found 1 implementation(s)"),
        "{}",
        text_of(&result)
    );

    let empty_remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/implementation" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let none = execute_tool(
        empty_remote,
        &ws.root(),
        "code_implementations",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 11 }),
    )
    .await
    .expect("the query runs");
    assert_eq!(text_of(&none), "No implementations found.");
}

#[tokio::test]
async fn code_outline_lists_symbols_and_hides_locals_by_default() {
    let ws = workspace();
    write(
        &ws,
        "src/lib.rs",
        "pub fn a() {\n    let x = 1;\n}\n\npub static COUNT: u32 = 1;\n",
    );
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        // The analyzer reports a local and a top-level `static` with the same kind, 13.
        "textDocument/documentSymbol" => serde_json::json!([
            answers::document_symbol("a", 12, 1, 3, 8),
            { "name": "x", "kind": 13,
              "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 14 } } },
            { "name": "COUNT", "kind": 13,
              "range": { "start": { "line": 4, "character": 0 }, "end": { "line": 4, "character": 26 } } }
        ]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_outline",
        serde_json::json!({ "path": "src/lib.rs" }),
    )
    .await
    .expect("the outline runs");
    let text = text_of(&result);
    assert!(text.contains("[Function] a (line 1)"), "{text}");
    assert!(!text.contains("[Variable] x"), "locals are hidden: {text}");
    assert!(
        text.contains("[Variable] COUNT (line 5)"),
        "a static is not a local: {text}"
    );
    assert!(text.contains("1 local variable(s) hidden"), "{text}");

    let with_locals = execute_tool(
        remote,
        &ws.root(),
        "code_outline",
        serde_json::json!({ "path": "src/lib.rs", "include_locals": true }),
    )
    .await
    .expect("the outline runs");
    assert!(text_of(&with_locals).contains("[Variable] x"));
}

#[tokio::test]
async fn code_outline_outlines_a_directory_skipping_files_it_cannot_outline() {
    let ws = workspace();
    write(&ws, "src/pkg/a.rs", "pub fn alpha() {}\n");
    write(&ws, "src/pkg/b.rs", "pub fn beta() {}\n");
    write(&ws, "src/pkg/README.md", "# Package\nDocs.\n");
    commit(&ws);

    let remote = scripted_gateway(Arc::new(|method, params| match method {
        "textDocument/documentSymbol" => {
            let uri = params
                .get("textDocument")
                .and_then(|t| t.get("uri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if uri.ends_with("a.rs") {
                serde_json::json!([answers::document_symbol("alpha", 12, 1, 1, 15)])
            } else if uri.ends_with("b.rs") {
                serde_json::json!([answers::document_symbol("beta", 12, 1, 1, 14)])
            } else if uri.ends_with("README.md") {
                // What rust-analyzer answered for Markdown it had parsed as Rust (#247).
                serde_json::json!([answers::document_symbol("or", 11, 80, 80, 13)])
            } else {
                serde_json::Value::Null
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await;

    let result = execute_tool(
        remote,
        &ws.root(),
        "code_outline",
        serde_json::json!({ "path": "src/pkg" }),
    )
    .await
    .expect("the outline runs");

    let text = text_of(&result);
    assert!(text.contains("Outline for src/pkg/a.rs:"), "{text}");
    assert!(text.contains("[Function] alpha (line 1)"), "{text}");
    assert!(text.contains("Outline for src/pkg/b.rs:"), "{text}");
    assert!(text.contains("[Function] beta (line 1)"), "{text}");
    assert!(!text.contains("README"), "{text}");
    assert!(text.contains("2 file(s) outlined, 1 skipped"), "{text}");
}

#[tokio::test]
async fn code_hover_and_type_at_render_markdown_or_say_there_is_none() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn a() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/hover" => answers::hover("```rust\npub fn a()\n```"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let hover = execute_tool(
        remote,
        &ws.root(),
        "code_hover",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("hover runs");
    assert!(
        text_of(&hover).contains("pub fn a()"),
        "{}",
        text_of(&hover)
    );

    let type_at = execute_tool(
        remote,
        &ws.root(),
        "code_type_at",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("type_at is an alias for hover");
    assert!(text_of(&type_at).contains("pub fn a()"));

    let empty_remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/hover" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let none = execute_tool(
        empty_remote,
        &ws.root(),
        "code_hover",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("hover runs");
    assert_eq!(text_of(&none), "No hover information available.");
}

// ---------------------------------------------------------------------------------------
// Symbol resolution (`symbol` instead of path/line/character), shared by many tools.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn a_symbol_argument_is_resolved_before_the_tool_runs() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn record() {}\n");
    commit(&ws);
    let path = lib.clone();
    let def_path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "workspace/symbol" => {
            serde_json::Value::Array(vec![answers::symbol("record", 12, &path, 1, 8)])
        }
        "textDocument/definition" => answers::locations(&def_path, &[(9, 1)]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({ "symbol": "record" }),
    )
    .await
    .expect("the symbol resolves and the definition query runs");
    assert!(text_of(&result).contains(":9:1"), "{}", text_of(&result));
}

#[tokio::test]
async fn an_unknown_symbol_name_is_reported() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn a() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "workspace/symbol" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_definition",
        serde_json::json!({ "symbol": "does_not_exist" }),
    )
    .await
    .expect_err("nothing resolves");
    assert!(
        format!("{err:#}").contains("no symbol named `does_not_exist`"),
        "{err:#}"
    );
}

#[tokio::test]
async fn a_symbol_name_with_two_unrelated_hits_is_ambiguous() {
    let ws = workspace();
    let a = write(&ws, "src/a.rs", "pub fn record() {}\n");
    let b = write(&ws, "src/b.rs", "pub fn record() {}\n");
    commit(&ws);
    let (a, b) = (a.clone(), b.clone());
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "workspace/symbol" => serde_json::Value::Array(vec![
            answers::symbol("record", 12, &a, 1, 8),
            answers::symbol("record", 12, &b, 1, 8),
        ]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_hover",
        serde_json::json!({ "symbol": "record" }),
    )
    .await
    .expect_err("two unrelated hits with the same score cannot be resolved");
    assert!(format!("{err:#}").contains("is ambiguous"), "{err:#}");
}

/// The index answers a name with fuzzy matches too. A name that only a fuzzy match answers is
/// not resolved to it (#253): the error names the closest names instead, nearest first.
#[tokio::test]
async fn a_name_only_fuzzy_hits_answer_is_refused_with_the_closest_names() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "pub fn query_with_text_opens_a_private_overlay() {}\npub fn relay() {}\n",
    )]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("query_with_text_opens_a_private_overlay", 12, &lib, 1, 8),
            answers::symbol("relay", 12, &lib, 2, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let err = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "relaid_files", None)
        .await
        .expect_err("no hit is called `relaid_files`");
    assert_eq!(
        format!("{err:#}"),
        "no symbol named `relaid_files` in the workspace index; did you mean: relay, \
         query_with_text_opens_a_private_overlay"
    );
}

/// A struct field is not in rust-analyzer's index. `Type::field` resolves through the outline
/// of the type's file, as a child of the type, and `Type::method` as a child of an `impl`
/// block for it; both the nested outline of a language server and the flat one of the
/// gateway's own engine are read. A fuzzy hit for the member's name is not taken.
#[tokio::test]
async fn a_qualified_field_resolves_through_the_types_outline() {
    let source = "pub struct RemoteOutcome {\n    pub relaid_files: usize,\n}\n\nimpl RemoteOutcome {\n    pub fn merge(&mut self) {}\n}\n\npub fn relay_files_elsewhere() {}\n";
    let ws = Workspace::new(&[("src/lib.rs", source)]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");

    let index = {
        let lib = lib.clone();
        move |params: &serde_json::Value| match params.get("query").and_then(|q| q.as_str()) {
            Some("relaid_files") => {
                serde_json::json!([answers::symbol("relay_files_elsewhere", 12, &lib, 9, 8)])
            }
            Some("RemoteOutcome") => {
                serde_json::json!([answers::symbol("RemoteOutcome", 23, &lib, 1, 12)])
            }
            _ => serde_json::json!([]),
        }
    };
    let mut ty = answers::document_symbol("RemoteOutcome", 23, 1, 3, 12);
    ty["children"] = serde_json::json!([answers::document_symbol("relaid_files", 8, 2, 2, 9)]);
    let mut imp = answers::document_symbol("impl RemoteOutcome", 19, 5, 7, 6);
    imp["children"] = serde_json::json!([answers::document_symbol("merge", 6, 6, 6, 12)]);
    let nested = serde_json::json!([
        ty,
        imp,
        answers::document_symbol("relay_files_elsewhere", 12, 9, 9, 8)
    ]);
    let script = index.clone();
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => script(params),
        "textDocument/documentSymbol" => nested.clone(),
        _ => serde_json::Value::Null,
    })
    .await;
    let field = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "RemoteOutcome::relaid_files",
        None,
    )
    .await
    .expect("the field is a child of the type");
    assert_eq!(
        (field.path.clone(), field.line, field.col, field.kind),
        (lib.clone(), 2, 9, "Field")
    );
    let method =
        prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "RemoteOutcome::merge", None)
            .await
            .expect("the method is a child of the type's impl block");
    assert_eq!((method.line, method.col), (6, 12));

    let uri = format!("file://{}", lib.display());
    let flat_entry = |name: &str, kind: u32, line: u32, col: u32, container: &str| {
        serde_json::json!({
            "name": name,
            "kind": kind,
            "location": { "uri": uri, "range": {
                "start": { "line": line - 1, "character": col - 1 },
                "end": { "line": line - 1, "character": 0 }
            } },
            "containerName": container
        })
    };
    let flat = serde_json::json!([
        flat_entry("RemoteOutcome", 23, 1, 12, "pub struct RemoteOutcome"),
        flat_entry("relaid_files", 8, 2, 9, "RemoteOutcome"),
        flat_entry("impl RemoteOutcome", 19, 5, 6, "impl RemoteOutcome"),
        flat_entry("merge", 6, 6, 12, "impl RemoteOutcome"),
    ]);
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "workspace/symbol" => index(params),
        "textDocument/documentSymbol" => flat.clone(),
        _ => serde_json::Value::Null,
    })
    .await;
    let field = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "RemoteOutcome::relaid_files",
        None,
    )
    .await
    .expect("a flat outline names the field's type as its container");
    assert_eq!((field.path, field.line, field.col), (lib.clone(), 2, 9));
    let method =
        prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "RemoteOutcome::merge", None)
            .await
            .expect("a flat outline names the impl block as the method's container");
    assert_eq!((method.line, method.col), (6, 12));
}

/// Among several hits, the one whose name is exactly the requested one wins: a prefix match
/// (`records`) is not a candidate, and a hit that differs only in case (`Record`) loses to the
/// exact spelling while still answering its own.
#[tokio::test]
async fn an_exact_hit_wins_over_the_others() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "pub fn records() {}\npub fn record() {}\npub struct Record;\n",
    )]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");
    let path = lib.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("records", 12, &path, 1, 8),
            answers::symbol("record", 12, &path, 2, 8),
            answers::symbol("Record", 23, &path, 3, 12),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "record", None)
        .await
        .expect("the exact spelling wins");
    assert_eq!((hit.path, hit.line, hit.col), (lib.clone(), 2, 8));
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "Record", None)
        .await
        .expect("the exact spelling wins");
    assert_eq!((hit.line, hit.col), (3, 12));
}

/// A dependency type is listed twice by the index, at its definition and at the `pub use` that
/// re-exports it. Its source is only on the node, so the resolver reads it from the gateway to
/// tell them apart, and the answer is the definition (#271).
#[tokio::test]
async fn a_dependency_type_resolves_past_its_reexport_read_from_the_node() {
    let ws = Workspace::new(&[("src/lib.rs", "pub fn local() {}\n")]);
    let root = ws.root();
    let registry = std::path::PathBuf::from("/nonexistent-registry/tokio-util/src/codec");
    let definition = registry.join("framed.rs");
    let reexport = registry.join("mod.rs");
    let mut source = vec![String::new(); 400];
    source[37] = "pub struct Framed<T, U> {".to_string();
    source[340] = "pub use self::framed::{Framed, FramedParts};".to_string();
    let source = source.join("\n");
    let (def, re) = (definition.clone(), reexport.clone());
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("Framed", 23, &re, 341, 24),
            answers::symbol("Framed", 23, &def, 38, 12),
        ]),
        "prod-code/readFile" => serde_json::json!(source),
        "textDocument/hover" => answers::hover("```rust\npub struct Framed<T, U>\n```"),
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "Framed", None)
        .await
        .expect("the re-export is not a second candidate");
    assert_eq!((hit.path, hit.line, hit.col), (definition, 38, 12));

    // A position query on that file is sent without opening it here: the node's analyzer
    // already has it, and reading it on this machine would fail.
    let hover = execute_tool(
        gateway.addr(),
        &root,
        "code_hover",
        serde_json::json!({ "symbol": "Framed" }),
    )
    .await
    .expect("a hover on a file that exists only on the node");
    assert!(!hover.is_error, "{}", text_of(&hover));
}

/// A server that decorates the names it lists (`bar()`, `Api.baz`) still answers the bare
/// name, and `bar` is not confused with `barrel()`.
#[tokio::test]
async fn a_decorated_server_name_matches_the_bare_name() {
    let ws = Workspace::new(&[(
        "src/lib.rs",
        "pub fn bar() {}\npub fn barrel() {}\npub fn baz() {}\n",
    )]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");
    let path = lib.clone();
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("barrel()", 12, &path, 2, 8),
            answers::symbol("bar()", 12, &path, 1, 8),
            answers::symbol("Api.baz", 6, &path, 3, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "bar", None)
        .await
        .expect("`bar()` is `bar`");
    assert_eq!((hit.path, hit.line, hit.col), (lib.clone(), 1, 8));
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "baz", None)
        .await
        .expect("`Api.baz` is `baz`");
    assert_eq!((hit.line, hit.col), (3, 8));
}

// ---------------------------------------------------------------------------------------
// Diagnostics and edit validation.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn code_diagnostics_reports_errors_and_warnings() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn a() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/diagnostic" => answers::error_at(1, 8, "E0308", "mismatched types"),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_diagnostics",
        serde_json::json!({ "path": "src/lib.rs" }),
    )
    .await
    .expect("the query runs");
    assert!(result.is_error);
    let text = text_of(&result);
    assert!(text.contains("1 error(s), 0 warning(s)"), "{text}");
    assert!(text.contains("E0308"), "{text}");
}

#[tokio::test]
async fn code_validate_edit_checks_proposed_text_without_writing() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn a() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edit",
        serde_json::json!({ "path": "src/lib.rs", "new_text": "pub fn b() {}\n" }),
    )
    .await
    .expect("validation runs");
    assert!(!result.is_error);
    assert!(text_of(&result).contains("0 error(s), 0 warning(s)"));
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), "pub fn a() {}\n");
}

/// Every session a validation opens, the one that reads the file as it is on disk included,
/// asks for the validation engine: that is the engine the gateway warms, and the main engine
/// is cold for a large file's diagnostics after a restart (#235).
#[tokio::test]
async fn a_validation_asks_only_the_validation_engine() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn a() {}\n");
    write(&ws, "src/other.rs", "pub fn b() {}\n");
    commit(&ws);
    let purposes: Arc<std::sync::Mutex<Vec<Option<String>>>> = Arc::default();
    let seen = Arc::clone(&purposes);
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "prod-code/handshake" => {
            let purpose = params
                .get("purpose")
                .and_then(|p| p.as_str())
                .map(str::to_string);
            seen.lock().unwrap().push(purpose);
            serde_json::Value::Null
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    for (tool, args) in [
        (
            "code_validate_edit",
            serde_json::json!({ "path": "src/lib.rs", "new_text": "pub fn b() {}\n" }),
        ),
        (
            "code_validate_edits",
            serde_json::json!({
                "edits": [ { "path": "src/lib.rs", "new_text": "pub fn a2() {}\n" } ],
                "also_check": [ "src/other.rs" ]
            }),
        ),
    ] {
        let result = execute_tool(remote, &ws.root(), tool, args)
            .await
            .expect("validation runs");
        assert!(!result.is_error, "{}", text_of(&result));
    }
    let purposes = purposes.lock().unwrap().clone();
    assert!(
        purposes.len() >= 4,
        "two sessions per validation: {purposes:?}"
    );
    assert!(
        purposes
            .iter()
            .all(|p| p.as_deref() == Some(prod_code_protocol::PURPOSE_VALIDATION)),
        "{purposes:?}"
    );
}

#[tokio::test]
async fn code_validate_edits_checks_several_files_together() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn a() {}\n");
    write(&ws, "src/other.rs", "pub fn b() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({
            "edits": [
                { "path": "src/lib.rs", "new_text": "pub fn a2() {}\n" }
            ],
            "also_check": [ "src/other.rs" ]
        }),
    )
    .await
    .expect("validation runs");
    assert!(!result.is_error);
    assert!(text_of(&result).contains("2 file(s) checked together: 0 error(s), 0 warning(s)"));
}

/// clangd judges a source against the header text that is open when it builds the source, and
/// asking about a source builds it. A header listed after its sources was opened too late: the
/// sources kept the old prototype's errors (#292). Its proposed text goes to the analyzer
/// first, and the report keeps the order the edits were given in.
#[tokio::test]
async fn code_validate_edits_opens_a_changed_header_before_the_sources_that_include_it() {
    let ws = workspace();
    write(&ws, "src/pricing.h", "int price(int qty);\n");
    write(
        &ws,
        "src/pricing.c",
        "#include \"pricing.h\"\nint price(int qty) { return qty * 80; }\n",
    );
    write(
        &ws,
        "src/main.c",
        "#include \"pricing.h\"\nint main(void) { return price(3); }\n",
    );
    commit(&ws);
    let opened: Arc<std::sync::Mutex<Vec<(String, String)>>> = Arc::default();
    let seen = Arc::clone(&opened);
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        // A file the session has already asked about is open with its disk text, and its
        // proposed text arrives as a change.
        "textDocument/didOpen" | "textDocument/didChange" => {
            let doc = &params["textDocument"];
            let uri = doc["uri"].as_str().unwrap_or_default();
            let name = uri.rsplit('/').next().unwrap_or_default().to_string();
            let text = doc["text"]
                .as_str()
                .or_else(|| params.pointer("/contentChanges/0/text")?.as_str())
                .unwrap_or_default()
                .to_string();
            seen.lock().unwrap().push((name, text));
            serde_json::Value::Null
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({
            "edits": [
                { "path": "src/main.c",
                  "new_text": "#include \"pricing.h\"\nint main(void) { return price(3, 80); }\n" },
                { "path": "src/pricing.c",
                  "new_text": "#include \"pricing.h\"\nint price(int qty, int rate) { return qty * rate; }\n" },
                { "path": "src/pricing.h", "new_text": "int price(int qty, int rate);\n" }
            ]
        }),
    )
    .await
    .expect("validation runs");
    let text = text_of(&result);
    assert!(!result.is_error, "{text}");
    let proposed: Vec<String> = opened
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, text)| text.contains("rate"))
        .map(|(name, _)| name.clone())
        .collect();
    assert_eq!(
        proposed.first().map(String::as_str),
        Some("pricing.h"),
        "the header's proposed text is open before any source is: {proposed:?}"
    );
    let at = |file: &str| text.find(&format!("{file}: ")).expect(file);
    assert!(
        at("src/main.c") < at("src/pricing.c") && at("src/pricing.c") < at("src/pricing.h"),
        "{text}"
    );
}

/// A new file has nothing on disk to compare with; the analyzer's "type annotations needed" on
/// its `#[derive(Deserialize)]` line is still not an error of the edit (#159).
#[tokio::test]
async fn a_new_file_is_not_refused_for_the_analyzers_derive_expansion() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub mod probe;\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/diagnostic" => serde_json::json!({ "kind": "full", "items": [
            { "severity": 1, "code": "E0282", "message": "type annotations needed",
              "range": { "start": { "line": 2, "character": 2 }, "end": { "line": 2, "character": 8 } } }
        ] }),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({
            "edits": [ {
                "path": "src/probe.rs",
                "new_text": "use serde::Deserialize;\n\n#[derive(Debug, Deserialize)]\npub struct Probe {\n    pub a: u32,\n}\n"
            } ]
        }),
    )
    .await
    .expect("validation runs");
    let text = text_of(&result);
    assert!(!result.is_error, "{text}");
    assert!(text.contains("src/probe.rs: 0 error(s)"), "{text}");
    assert!(
        text.contains("1 \"type annotations needed\" on a #[derive(...)] line are not counted"),
        "{text}"
    );
}

// ---------------------------------------------------------------------------------------
// Refactors: rename, safe delete, assists, codemod.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn code_rename_writes_the_analyzers_edit_into_the_checkout() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn old_name() {}\n");
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/rename" => {
            answers::whole_file(&path, "pub fn old_name() {}\n", "pub fn new_name() {}\n")
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_rename",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8, "new_name": "new_name" }),
    )
    .await
    .expect("the rename runs");
    assert!(!result.is_error);
    assert!(
        text_of(&result).contains("renamed to `new_name`; 1 path(s)"),
        "{}",
        text_of(&result)
    );
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "pub fn new_name() {}\n"
    );
}

/// With `comments`, the old name follows the rename into the comments and the test names of the
/// file, in the same written change; without it, only the analyzer's edit is written (#174).
#[tokio::test]
async fn a_rename_with_comments_follows_the_old_name_into_prose_and_tests() {
    let ws = workspace();
    let before = "/// An `Order` is priced once.\npub struct Order;\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn order_is_priced() {\n        let _ = super::Order;\n    }\n}\n";
    let renamed = before
        .replace("pub struct Order;", "pub struct Trade;")
        .replace("super::Order", "super::Trade");
    let lib = write(&ws, "src/lib.rs", before);
    commit(&ws);
    let (path, renamed_text) = (lib.clone(), renamed.clone());
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/rename" => answers::whole_file(&path, before, &renamed_text),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_rename",
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 12, "new_name": "Trade", "comments": true }),
    )
    .await
    .expect("the rename runs");
    let text = text_of(&result);
    assert!(!result.is_error, "{text}");
    assert!(
        text.contains("in comments: 1 mention(s) of the old name replaced"),
        "{text}"
    );
    assert!(
        text.contains("test renamed: `order_is_priced` -> `trade_is_priced`"),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "/// An `Trade` is priced once.\npub struct Trade;\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn trade_is_priced() {\n        let _ = super::Trade;\n    }\n}\n"
    );
}

/// A rename to a name already declared in the same scope is a second definition, not a
/// rename, and the analyzer computes it without complaint (#98). The result is checked before
/// it is written: refused with the error, and written only with `force`, which says so.
#[tokio::test]
async fn a_rename_that_does_not_compile_is_refused_unless_forced() {
    let ws = workspace();
    let before = "pub fn a() {}\npub fn b() {}\n";
    let lib = write(&ws, "src/lib.rs", before);
    commit(&ws);
    let path = lib.clone();
    // The first pull of each check is the file on disk, the second the proposal.
    let pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/rename" => {
            answers::whole_file(&path, before, "pub fn a() {}\npub fn a() {}\n")
        }
        "textDocument/diagnostic"
            if pulls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .is_multiple_of(2) =>
        {
            answers::no_diagnostics()
        }
        "textDocument/diagnostic" => {
            answers::error_at(2, 8, "E0428", "the name `a` is defined multiple times")
        }
        _ => serde_json::Value::Null,
    }))
    .await;

    let refused = execute_tool(
        remote,
        &ws.root(),
        "code_rename",
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 8, "new_name": "a" }),
    )
    .await
    .expect("the rename runs");
    let text = text_of(&refused);
    assert!(refused.is_error, "{text}");
    assert!(text.contains("refused") && text.contains("E0428"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        before,
        "nothing written"
    );

    let forced = execute_tool(
        remote,
        &ws.root(),
        "code_rename",
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 8, "new_name": "a", "force": true }),
    )
    .await
    .expect("the rename runs");
    let text = text_of(&forced);
    assert!(!forced.is_error, "{text}");
    assert!(
        text.contains("written with `force`") && text.contains("E0428"),
        "{text}"
    );
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "pub fn a() {}\npub fn a() {}\n"
    );
}

#[tokio::test]
async fn code_rename_with_no_edits_is_reported() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn old_name() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/rename" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_rename",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8, "new_name": "x" }),
    )
    .await
    .expect("the rename runs");
    assert!(result.is_error);
    assert_eq!(text_of(&result), "rename produced no edits");
}

#[tokio::test]
async fn code_safe_delete_removes_an_unreferenced_item() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn unused() {}\n");
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "prodCode/safeDelete" => answers::whole_file(&path, "pub fn unused() {}\n", ""),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_safe_delete",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8 }),
    )
    .await
    .expect("the delete runs");
    assert!(!result.is_error);
    assert!(
        text_of(&result).contains("deleted; 1 path(s)"),
        "{}",
        text_of(&result)
    );
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), "");
}

const WITH_PRELUDE: &str = "fn g() -> std::prelude::v1::Option<u8> {\n    None\n}\n";

/// A file that already mentions the prelude path — in a comment, or code that spells it on
/// purpose — keeps those lines as they are; only the lines the assist wrote are shortened.
#[tokio::test]
async fn only_the_lines_an_assist_wrote_are_respelled() {
    let ws = workspace();
    let before = "// std::prelude::v1::Option is spelled out here on purpose\nfn g() {}\n";
    let lib = write(&ws, "src/lib.rs", before);
    commit(&ws);
    let path = lib.clone();
    let after =
        format!("// std::prelude::v1::Option is spelled out here on purpose\n{WITH_PRELUDE}");
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(&path, before, &after),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_assist",
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 4, "id": "some_assist" }),
    )
    .await
    .expect("the assist runs");
    assert!(
        text_of(&result).contains("1 `std::prelude::v1::` path(s)"),
        "{}",
        text_of(&result)
    );
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "// std::prelude::v1::Option is spelled out here on purpose\nfn g() -> Option<u8> {\n    None\n}\n"
    );
}

/// An assist that spells a prelude item by its full path gets the name in scope instead, when the
/// analyzer accepts it (#97), and keeps rust-analyzer's spelling when it does not.
#[tokio::test]
async fn an_assists_prelude_path_is_shortened_only_when_the_analyzer_accepts_it() {
    for accepted in [true, false] {
        let ws = workspace();
        let lib = write(&ws, "src/lib.rs", "fn g() {}\n");
        commit(&ws);
        let path = lib.clone();
        let pulls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let remote = scripted_gateway(Arc::new(move |method, _| match method {
            "prodCode/applyAssist" => answers::whole_file(&path, "fn g() {}\n", WITH_PRELUDE),
            "textDocument/diagnostic"
                if accepted
                    || pulls
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        .is_multiple_of(2) =>
            {
                answers::no_diagnostics()
            }
            "textDocument/diagnostic" => {
                answers::error_at(1, 11, "E0412", "cannot find type `Option` in this scope")
            }
            _ => serde_json::Value::Null,
        }))
        .await;
        let result = execute_tool(
            remote,
            &ws.root(),
            "code_assist",
            serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 4, "id": "some_assist" }),
        )
        .await
        .expect("the assist runs");
        let text = text_of(&result);
        let written = std::fs::read_to_string(&lib).unwrap();
        if accepted {
            assert_eq!(written, "fn g() -> Option<u8> {\n    None\n}\n");
            assert!(text.contains("1 `std::prelude::v1::` path(s)"), "{text}");
        } else {
            assert_eq!(written, WITH_PRELUDE, "the analyzer's spelling is kept");
            assert!(!text.contains("std::prelude"), "{text}");
        }
    }
}

#[tokio::test]
async fn code_assists_lists_actions_or_says_there_are_none() {
    let ws = workspace();
    write(&ws, "src/lib.rs", "pub fn a() { 1 + 1; }\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "prodCode/assists" => serde_json::json!([
            { "id": "extract_variable", "kind": "refactor.extract", "label": "Extract into variable" }
        ]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_assists",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 14 }),
    )
    .await
    .expect("the query runs");
    assert!(
        text_of(&result).contains("extract_variable [refactor.extract]: Extract into variable"),
        "{}",
        text_of(&result)
    );

    let empty_remote = scripted_gateway(Arc::new(|method, _| match method {
        "prodCode/assists" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let none = execute_tool(
        empty_remote,
        &ws.root(),
        "code_assists",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 14 }),
    )
    .await
    .expect("the query runs");
    assert!(text_of(&none).contains("no code actions at this position"));
}

#[tokio::test]
async fn code_assist_applies_the_chosen_action() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn a() { 1 + 1; }\n");
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(
            &path,
            "pub fn a() { 1 + 1; }\n",
            "pub fn a() { let x = 1 + 1; }\n",
        ),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_assist",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 14, "id": "extract_variable" }),
    )
    .await
    .expect("the assist runs");
    assert!(!result.is_error);
    assert!(
        text_of(&result).contains("applied `extract_variable`; 1 path(s)"),
        "{}",
        text_of(&result)
    );
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "pub fn a() { let x = 1 + 1; }\n"
    );
}

#[tokio::test]
async fn code_codemod_reports_no_match_and_applies_a_match() {
    let ws = workspace();
    let lib = write(
        &ws,
        "src/lib.rs",
        "fn a(x: Option<i32>) -> i32 {\n    x.unwrap()\n}\n",
    );
    commit(&ws);
    let path = lib.clone();
    let no_match_remote = scripted_gateway(Arc::new(|method, _| match method {
        "prodCode/structuralReplace" => serde_json::Value::Null,
        _ => serde_json::Value::Null,
    }))
    .await;
    let no_match = execute_tool(
        no_match_remote,
        &ws.root(),
        "code_codemod",
        serde_json::json!({ "rule": "$a.ok() ==>> $a.ok_or(())", "path": "src/lib.rs" }),
    )
    .await
    .expect("the rule runs");
    assert!(
        text_of(&no_match).contains("matches nothing"),
        "{}",
        text_of(&no_match)
    );

    let old = "fn a(x: Option<i32>) -> i32 {\n    x.unwrap()\n}\n";
    let new = "fn a(x: Option<i32>) -> i32 {\n    x.expect(\"invariant\")\n}\n";
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "prodCode/structuralReplace" => answers::whole_file(&path, old, new),
        _ => serde_json::Value::Null,
    }))
    .await;
    let applied = execute_tool(
        remote,
        &ws.root(),
        "code_codemod",
        serde_json::json!({ "rule": "$a.unwrap() ==>> $a.expect(\"invariant\")", "path": "src/lib.rs", "apply": true }),
    )
    .await
    .expect("the rule runs");
    let text = text_of(&applied);
    assert!(text.contains("changed line(s) in 1 file(s)"), "{text}");
    assert!(text.contains("[applied to 1 file(s)"), "{text}");
    assert_eq!(std::fs::read_to_string(&lib).unwrap(), new);
}

// ---------------------------------------------------------------------------------------
// The higher-level refactors that delegate to their own modules: schema rename, change
// signature, generate fixture. The fixtures mirror the module-level tests in
// `tests/orchestration.rs`; here they are driven through `execute_tool`.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn code_schema_rename_renames_a_field_and_reports_clean() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let lib = write(
        &ws,
        "src/lib.rs",
        "pub struct Order {\n    pub order_id: String,\n}\n",
    );
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/rename" => answers::whole_file(
            &path,
            "pub struct Order {\n    pub order_id: String,\n}\n",
            "pub struct Order {\n    pub trade_id: String,\n}\n",
        ),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_schema_rename",
        serde_json::json!({ "field": "order_id", "to": "trade_id" }),
    )
    .await
    .expect("the rename runs");
    assert!(!result.is_error);
    let text = text_of(&result);
    assert!(text.contains("`order_id` → `trade_id`"), "{text}");
    assert!(
        !text.contains("[applied"),
        "a dry run says nothing about applying"
    );
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "pub struct Order {\n    pub order_id: String,\n}\n",
        "a dry run writes nothing"
    );
}

#[tokio::test]
async fn code_change_signature_reorders_parameters_and_call_sites() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n\npub fn use_it() -> String {\n    join(\"x\", \"y\")\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "prodCode/structuralReplace" => answers::whole_file(
            &path,
            source,
            "pub fn join(a: &str, b: &str) -> String {\n    format!(\"{a}{b}\")\n}\n\npub fn use_it() -> String {\n    join(\"y\", \"x\")\n}\n",
        ),
        "textDocument/references" => serde_json::json!([
            { "uri": format!("file://{}", path.display()),
              "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 8 } } }
        ]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_change_signature",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "character": 8, "params": ["b", "a"] }),
    )
    .await
    .expect("the change runs");
    assert!(!result.is_error);
    let text = text_of(&result);
    assert!(text.contains("- now: (b: &str, a: &str)"), "{text}");
    assert!(text.contains("join(\"y\", \"x\")"), "{text}");
}

#[tokio::test]
async fn code_generate_fixture_builds_and_verifies_a_value() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let source = "pub struct Config {\n    pub name: String,\n    pub retries: u32,\n}\n";
    let lib = write(&ws, "src/lib.rs", source);
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            { "name": "Config", "kind": 23,
              "location": { "uri": format!("file://{}", path.display()),
                            "range": { "start": { "line": 0, "character": 11 },
                                       "end": { "line": 0, "character": 17 } } } }
        ]),
        "textDocument/documentSymbol" => serde_json::json!([
            { "name": "Config", "kind": 23,
              "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 3, "character": 1 } },
              "selectionRange": { "start": { "line": 0, "character": 11 }, "end": { "line": 0, "character": 17 } } }
        ]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_generate_fixture",
        serde_json::json!({ "symbol": "Config" }),
    )
    .await
    .expect("the fixture is built");
    assert!(!result.is_error);
    assert!(
        text_of(&result).contains("the analyzer accepts it: 0 errors"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn code_slice_returns_the_seed_declaration_at_depth_zero() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "fn compute() -> i32 {\n    1\n}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("compute", 12, 1, 3, 4)])
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_slice",
        serde_json::json!({ "path": "src/lib.rs", "line": 1, "depth": 0 }),
    )
    .await
    .expect("the slice runs");
    let text = text_of(&result);
    assert!(text.contains("slice of `compute`: 1 item(s)"), "{text}");
}

#[tokio::test]
async fn code_dead_code_finds_an_unreferenced_function() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "fn helper() -> i32 {\n    1\n}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("helper", 12, 1, 3, 4)])
        }
        "textDocument/references" => serde_json::json!([]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(remote, &ws.root(), "code_dead_code", serde_json::json!({}))
        .await
        .expect("the scan runs");
    let text = text_of(&result);
    assert!(text.contains("1 unreferenced"), "{text}");
    assert!(text.contains("function helper"), "{text}");
}

#[tokio::test]
async fn code_impact_attributes_a_changed_line_to_its_function() {
    let ws = rust_workspace("pub fn a() -> i32 {\n    1\n}\n");
    write(&ws, "src/lib.rs", "pub fn a() -> i32 {\n    10\n}\n");
    let remote = scripted_gateway(Arc::new(|method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("a", 12, 1, 3, 8)])
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_impact",
        serde_json::json!({ "depth": 0 }),
    )
    .await
    .expect("the analysis runs");
    let text = text_of(&result);
    assert!(
        text.contains("1 changed file(s), 1 changed function(s)"),
        "{text}"
    );
    assert!(text.contains("changed functions:"), "{text}");
    assert!(text.contains("• a"), "{text}");
    assert!(
        text.contains("affected tests: none reach the changed functions"),
        "{text}"
    );
}

/// A Python server that has just started answers the call hierarchy with nothing for a while.
/// `impact` asks again before it believes "no callers", and the caller is found once the server
/// has read the project (#202).
#[tokio::test]
async fn code_impact_asks_a_cold_server_again_before_it_reports_no_callers() {
    let ws = workspace();
    write(&ws, "pyproject.toml", "[project]\nname = \"shop\"\n");
    let pricing = write(&ws, "shop/pricing.py", "def price(x):\n    return x\n");
    let checks = write(
        &ws,
        "checks/check_price.py",
        "from shop.pricing import price\n\n\ndef test_doubles():\n    assert price(2) == 2\n",
    );
    commit(&ws);
    write(&ws, "shop/pricing.py", "def price(x):\n    return x * 1\n");
    let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = asked.clone();
    let (p, c) = (
        format!("file://{}", pricing.display()),
        format!("file://{}", checks.display()),
    );
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("price", 12, 1, 2, 5)])
        }
        "textDocument/prepareCallHierarchy" => serde_json::json!([{ "name": "price", "uri": p,
            "selectionRange": { "start": { "line": 0, "character": 4 }, "end": { "line": 0, "character": 9 } } }]),
        "callHierarchy/incomingCalls" => {
            if counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) < 2 {
                serde_json::json!([])
            } else {
                serde_json::json!([{ "from": { "name": "test_doubles", "uri": c,
                    "selectionRange": { "start": { "line": 3, "character": 4 }, "end": { "line": 3, "character": 16 } } },
                    "fromRanges": [ { "start": { "line": 4, "character": 11 }, "end": { "line": 4, "character": 16 } } ] }])
            }
        }
        _ => serde_json::Value::Null,
    }))
    .await;
    let text = text_of(
        &execute_tool(remote, &ws.root(), "code_impact", serde_json::json!({}))
            .await
            .expect("the analysis runs"),
    );
    // `checks/check_price.py` is not a test file by pytest's naming, so the caller is a caller.
    assert!(text.contains("1 caller(s)"), "{text}");
    assert!(
        text.contains("test_doubles  checks/check_price.py:4:5"),
        "{text}"
    );
    assert!(
        asked.load(std::sync::atomic::Ordering::SeqCst) >= 3,
        "the empty answers were asked again"
    );
}

// ---------------------------------------------------------------------------------------
// Tools that speak the exec / search / shadow-run / status / read-file wire messages.
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn code_exec_reports_the_exit_status_and_output_tail() {
    let ws = rust_workspace("pub fn a() {}\n");
    let ok_remote = mock_gateway(Script {
        exec_stdout: b"hello from the gateway\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    })
    .await;
    let ok = execute_tool(
        ok_remote,
        &ws.root(),
        "code_exec",
        serde_json::json!({ "argv": ["echo", "hi"] }),
    )
    .await
    .expect("exec runs");
    assert!(!ok.is_error);
    let text = text_of(&ok);
    assert!(text.contains("$ echo hi"), "{text}");
    assert!(text.contains("exit code 0"), "{text}");
    assert!(text.contains("hello from the gateway"), "{text}");

    let fail_remote = mock_gateway(Script {
        exec_stderr: b"boom\n".to_vec(),
        exec_exit: Some(1),
        ..Script::default()
    })
    .await;
    let failed = execute_tool(
        fail_remote,
        &ws.root(),
        "code_exec",
        serde_json::json!({ "argv": ["false"] }),
    )
    .await
    .expect("exec runs");
    assert!(failed.is_error);
    assert!(text_of(&failed).contains("exit code 1"));
}

#[tokio::test]
async fn code_check_reports_ok_with_no_diagnostics() {
    let ws = rust_workspace("pub fn a() -> i32 {\n    1\n}\n");
    let remote = mock_gateway(Script {
        exec_exit: Some(0),
        ..Script::default()
    })
    .await;
    let result = execute_tool(remote, &ws.root(), "code_check", serde_json::json!({}))
        .await
        .expect("check runs");
    assert!(!result.is_error);
    assert!(
        text_of(&result).contains("rust check: OK"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn code_lint_reports_a_warning_diagnostic_and_fails_on_nonzero_exit() {
    let ws = rust_workspace("fn helper() {}\n");
    let stdout = "{\"reason\":\"compiler-message\",\"message\":{\"level\":\"warning\",\"message\":\"unused function\",\"code\":{\"code\":\"dead_code\"},\"spans\":[{\"is_primary\":true,\"file_name\":\"src/lib.rs\",\"line_start\":3,\"column_start\":1}]}}\n";
    let remote = mock_gateway(Script {
        exec_stdout: stdout.as_bytes().to_vec(),
        exec_exit: Some(1),
        ..Script::default()
    })
    .await;
    let result = execute_tool(remote, &ws.root(), "code_lint", serde_json::json!({}))
        .await
        .expect("lint runs");
    assert!(result.is_error);
    let text = text_of(&result);
    assert!(text.contains("[dead_code]"), "{text}");
    assert!(text.contains("unused function"), "{text}");
    assert!(text.contains("src/lib.rs:3:1"), "{text}");
}

#[tokio::test]
async fn code_test_parses_cargo_test_output() {
    let ws = rust_workspace("pub fn a() -> i32 {\n    1\n}\n");
    let remote = mock_gateway(Script {
        exec_stdout: b"test result: ok. 3 passed; 0 failed; 0 ignored\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    })
    .await;
    let result = execute_tool(remote, &ws.root(), "code_test", serde_json::json!({}))
        .await
        .expect("test runs");
    assert!(!result.is_error);
    assert!(
        text_of(&result).contains("3 passed, 0 failed"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn code_test_passes_env_streams_each_result_and_reports_usage() {
    let ws = rust_workspace("pub fn a() -> i32 {\n    1\n}\n");
    let usage = prod_code_protocol::ExecUsage {
        cpu_user_ms: 1200,
        cpu_sys_ms: 300,
        max_rss_kb: 51200,
    };
    let script = Script {
        exec_stdout: b"running 2 tests\ntest a::adds ... ok\ntest a::fails ... FAILED\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored\n".to_vec(),
        exec_exit: Some(101),
        exec_usage: Some(usage),
        ..Script::default()
    };
    let seen = script.exec_env.clone();
    let remote = mock_gateway(script).await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_test",
        serde_json::json!({ "env": { "RUST_BACKTRACE": "1" } }),
    )
    .await
    .expect("test runs");
    assert!(result.is_error);
    assert!(
        seen.lock()
            .unwrap()
            .contains(&("RUST_BACKTRACE".to_string(), "1".to_string()))
    );
    let text = text_of(&result);
    assert!(text.contains("1 passed, 1 failed"), "{text}");
    assert!(text.contains(&usage.render()), "{text}");

    for bad in [
        serde_json::json!({ "env": { "RUST_BACKTRACE": 1 } }),
        serde_json::json!({ "env": ["RUST_BACKTRACE=1"] }),
        serde_json::json!({ "env": { "A": "1" }, "fix": true, "timeout_secs": 1 }),
    ] {
        let tool = if bad.get("fix").is_some() {
            "code_check"
        } else {
            "code_test"
        };
        let err = execute_tool(remote, &ws.root(), tool, bad.clone())
            .await
            .expect_err(&format!("{bad} is refused"));
        assert!(format!("{err}").contains("env"), "{err}");
    }

    // Each result is handed over as its line arrives, though the lines come in pieces.
    let mut events = Vec::new();
    let report = prod_code_mcp::verify::run_verify_with(
        remote,
        &ws.root(),
        None,
        prod_code_mcp::verify::VerifyKind::Test,
        None,
        0,
        &[],
        |event| events.push(event),
    )
    .await
    .expect("test runs");
    use prod_code_mcp::verify::RunEvent;
    assert_eq!(
        events,
        vec![
            RunEvent::Test {
                name: "a::adds".into(),
                ok: true
            },
            RunEvent::Test {
                name: "a::fails".into(),
                ok: false
            },
        ]
    );
    assert_eq!(report.usage, Some(usage));
}

#[tokio::test]
async fn code_diagnose_failure_reports_clean_and_failing_runs() {
    let ws = rust_workspace("pub fn a() -> i32 {\n    1\n}\n");
    let clean_remote = mock_gateway(Script {
        exec_stdout: b"test result: ok. 2 passed; 0 failed\n".to_vec(),
        exec_exit: Some(0),
        ..Script::default()
    })
    .await;
    let clean = execute_tool(
        clean_remote,
        &ws.root(),
        "code_diagnose_failure",
        serde_json::json!({}),
    )
    .await
    .expect("diagnose runs");
    assert!(!clean.is_error);
    assert!(text_of(&clean).contains("2 passed, 0 failed"));
    assert!(text_of(&clean).contains("no failures"));

    let failing_stdout = "running 1 test\ntest test_foo ... FAILED\n\nfailures:\n\n---- test_foo stdout ----\nassertion failed\n\nfailures:\n    test_foo\n\ntest result: FAILED. 0 passed; 1 failed\n";
    let failing_remote = mock_gateway(Script {
        exec_stdout: failing_stdout.as_bytes().to_vec(),
        exec_exit: Some(101),
        ..Script::default()
    })
    .await;
    let failing = execute_tool(
        failing_remote,
        &ws.root(),
        "code_diagnose_failure",
        serde_json::json!({}),
    )
    .await
    .expect("diagnose runs");
    assert!(failing.is_error);
    let text = text_of(&failing);
    assert!(text.contains("0 passed, 1 failed"), "{text}");
    assert!(text.contains("test_foo"), "{text}");
}

#[tokio::test]
async fn code_search_renders_hits() {
    let ws = rust_workspace("pub fn foo() {}\n");
    let remote = mock_gateway(Script {
        search_hits: vec![SearchHit {
            file: "src/lib.rs".to_string(),
            line: 1,
            kind: "function".to_string(),
            name: "foo".to_string(),
            container: None,
            signature: "pub fn foo()".to_string(),
            doc: "does the thing".to_string(),
        }],
        ..Script::default()
    })
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_search",
        serde_json::json!({ "query": "does the thing" }),
    )
    .await
    .expect("the search runs");
    assert!(!result.is_error);
    assert!(text_of(&result).contains("foo"), "{}", text_of(&result));
}

#[tokio::test]
async fn code_shadow_run_ranks_and_can_apply_the_winner() {
    let ws = rust_workspace("pub fn a() -> i32 {\n    1\n}\n");
    let remote = mock_gateway(Script {
        shadow_results: vec![ShadowHypothesisResult {
            name: "h1".to_string(),
            exit_code: Some(0),
            duration_ms: 15,
            timed_out: false,
            error: None,
            output_tail: Some(b"test result: ok. 1 passed; 0 failed\n".to_vec()),
            output_len: 30,
        }],
        ..Script::default()
    })
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_shadow_run",
        serde_json::json!({
            "hypotheses": [ { "name": "h1", "edits": [ { "path": "src/lib.rs", "new_text": "pub fn a() -> i32 {\n    2\n}\n" } ] } ] ,
            "argv": ["cargo", "test"]
        }),
    )
    .await
    .expect("shadow run runs");
    assert!(!result.is_error);
    assert!(text_of(&result).contains("h1"), "{}", text_of(&result));

    let apply_remote = mock_gateway(Script {
        shadow_results: vec![ShadowHypothesisResult {
            name: "h1".to_string(),
            exit_code: Some(0),
            duration_ms: 15,
            timed_out: false,
            error: None,
            output_tail: Some(b"test result: ok. 1 passed; 0 failed\n".to_vec()),
            output_len: 30,
        }],
        ..Script::default()
    })
    .await;
    let applied = execute_tool(
        apply_remote,
        &ws.root(),
        "code_shadow_run",
        serde_json::json!({
            "hypotheses": [ { "name": "h1", "edits": [ { "path": "src/lib.rs", "new_text": "pub fn a() -> i32 {\n    2\n}\n" } ] } ],
            "argv": ["cargo", "test"],
            "apply": true
        }),
    )
    .await
    .expect("shadow run runs");
    assert!(!applied.is_error);
    assert_eq!(
        std::fs::read_to_string(ws.path("src/lib.rs")).unwrap(),
        "pub fn a() -> i32 {\n    2\n}\n"
    );
}

#[tokio::test]
async fn code_source_reads_a_file_from_the_gateway_host() {
    let ws = workspace();
    let remote = mock_gateway(Script {
        read_file: Some(b"line one\nline two\nline three\n".to_vec()),
        ..Script::default()
    })
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_source",
        serde_json::json!({ "path": "/usr/lib/rust/lib.rs", "line": 2, "context": 1 }),
    )
    .await
    .expect("read runs");
    let text = text_of(&result);
    assert!(text.contains("line two"), "{text}");

    let missing_remote = mock_gateway(Script {
        read_file: None,
        ..Script::default()
    })
    .await;
    let err = execute_tool(
        missing_remote,
        &ws.root(),
        "code_source",
        serde_json::json!({ "path": "/no/such/file" }),
    )
    .await
    .expect_err("a missing remote file is an error");
    assert!(format!("{err:#}").contains("no such file"), "{err:#}");
}

#[tokio::test]
async fn code_status_reports_health() {
    let ws = workspace();
    let remote = mock_gateway(Script::default()).await;
    let result = execute_tool(remote, &ws.root(), "code_status", serde_json::json!({}))
        .await
        .expect("status runs");
    let text = text_of(&result);
    assert!(text.contains("Status: HEALTHY"), "{text}");
    assert!(text.contains("Uptime: 2h 1m 5s"), "{text}");
}

#[tokio::test]
async fn code_sync_reports_the_delta() {
    let ws = rust_workspace("pub fn a() {}\n");
    let remote = scripted_gateway(Arc::new(|_, _| serde_json::Value::Null)).await;
    let result = execute_tool(remote, &ws.root(), "code_sync", serde_json::json!({}))
        .await
        .expect("sync runs");
    assert!(
        text_of(&result).contains("Fast-Sync Completed"),
        "{}",
        text_of(&result)
    );
}

const TWICE: &str = "pub fn a(x: u32) -> u32 {\n    let y = x * 2;\n    y + 1\n}\n\npub fn b(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n";
const TWICE_EXTRACTED: &str = "pub fn a(x: u32) -> u32 {\n    let y = fun_name(x);\n    y + 1\n}\n\nfn fun_name(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n\npub fn b(x: u32) -> u32 {\n    let y = x * 2;\n    y\n}\n";

/// `code_extract_function` with `duplicates: false` extracts the selection alone, names the
/// function, and writes with `apply` without asking the compiler (no duplicate was replaced).
#[tokio::test]
async fn extract_function_alone_names_the_function_and_writes() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", TWICE);
    commit(&ws);
    let path = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "prodCode/applyAssist" => answers::whole_file(&path, TWICE, TWICE_EXTRACTED),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let args = |apply: bool| {
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 5,
            "end_line": 2, "end_character": 19, "name": "doubled",
            "duplicates": false, "apply": apply })
    };
    let dry = execute_tool(remote, &ws.root(), "code_extract_function", args(false))
        .await
        .expect("the dry run reports");
    let text = text_of(&dry);
    assert!(text.contains("`fn doubled` extracted"), "{text}");
    assert!(
        text.contains("the selection now reads `let y = doubled(x);`"),
        "{text}"
    );
    assert!(
        !text.contains("compiler"),
        "no duplicate, no compiler: {text}"
    );
    assert_eq!(ws.read("src/lib.rs"), TWICE);

    let written = execute_tool(remote, &ws.root(), "code_extract_function", args(true))
        .await
        .expect("it writes");
    assert!(
        text_of(&written).contains("[applied]"),
        "{}",
        text_of(&written)
    );
    let now = ws.read("src/lib.rs");
    assert!(now.contains("fn doubled(x: u32) -> u32 {"), "{now}");
    assert!(
        now.contains("pub fn b(x: u32) -> u32 {\n    let y = x * 2;"),
        "{now}"
    );
}

/// `code_move_module` reports a module move, then writes it with `apply`: the file moves, the
/// declaration goes to the new parent, and the old file is gone.
#[tokio::test]
async fn move_module_reports_then_moves_the_file() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"mm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    write(&ws, "src/lib.rs", "pub mod a;\npub mod c;\n");
    write(&ws, "src/a.rs", "pub mod b;\n");
    write(&ws, "src/a/b.rs", "pub fn f() {}\n");
    write(&ws, "src/c.rs", "pub fn g() {}\n");
    commit(&ws);
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let args = |apply: bool| serde_json::json!({ "path": "src/a/b.rs", "to": "src/c/b.rs", "apply": apply });
    let dry = execute_tool(remote, &ws.root(), "code_move_module", args(false))
        .await
        .expect("the dry run reports");
    let text = text_of(&dry);
    assert!(text.contains("src/a/b.rs -> src/c/b.rs"), "{text}");
    assert!(text.contains("nothing was written"), "{text}");
    assert!(ws.root().join("src/a/b.rs").exists());

    let written = execute_tool(remote, &ws.root(), "code_move_module", args(true))
        .await
        .expect("it writes");
    assert!(
        text_of(&written).contains("[applied]"),
        "{}",
        text_of(&written)
    );
    assert!(!ws.root().join("src/a/b.rs").exists());
    assert_eq!(ws.read("src/c/b.rs"), "pub fn f() {}\n");
    assert_eq!(ws.read("src/c.rs"), "pub mod b;\n\npub fn g() {}\n");
    assert_eq!(ws.read("src/a.rs"), "");
}

const TRAIT_ONE: &str = "pub trait T {\n    fn f(&self, a: u8, b: u8) -> u8;\n}\n\npub struct S;\n\nimpl T for S {\n    fn f(&self, a: u8, _b: u8) -> u8 {\n        a\n    }\n}\n\npub fn g(s: &S) -> u8 {\n    s.f(1, 2)\n}\n";

/// `code_safe_delete` on a trait method's parameter removes it from the trait, the
/// implementation and the call, by position (#194).
#[tokio::test]
async fn safe_delete_removes_a_trait_method_parameter_everywhere() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", TRAIT_ONE);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/implementation" => answers::locations(&l, &[(8, 8)]),
        "textDocument/references" => answers::locations(&l, &[(8, 8), (14, 7)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_safe_delete",
        serde_json::json!({ "path": "src/lib.rs", "line": 2, "character": 24 }),
    )
    .await
    .expect("it runs");
    let text = text_of(&result);
    assert!(
        text.contains("leaves `T::f`: 2 declaration(s), 1 call(s)"),
        "{text}"
    );
    assert!(text.contains("[applied]"), "{text}");
    let now = ws.read("src/lib.rs");
    assert!(now.contains("fn f(&self, a: u8) -> u8;"), "{now}");
    assert!(now.contains("fn f(&self, a: u8) -> u8 {"), "{now}");
    assert!(now.contains("s.f(1)"), "{now}");
}

const MOVE_M: &str = "pub struct A {\n    pub n: u32,\n}\n\npub struct B {\n    pub m: u32,\n}\n\nimpl A {\n    pub fn sum(&self, b: &B) -> u32 {\n        self.n + b.m + { let b = 1; b }\n    }\n}\n\npub fn f(a: &A, b: &B) -> u32 {\n    a.sum(b)\n}\n";

/// `code_move_method` reports the move, and with no inherent `impl` for the new type makes one
/// right after the type's declaration.
#[tokio::test]
async fn move_method_makes_an_impl_when_the_type_has_none() {
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[package]\nname = \"mm\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let lib = write(&ws, "src/lib.rs", MOVE_M);
    commit(&ws);
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, params| match method {
        "textDocument/definition" => answers::locations(&l, &[(5, 12)]),
        // The parameter `b` (10:23) is used once; the inner `let b` is another binding (#207).
        "textDocument/references"
            if params
                .pointer("/position/character")
                .and_then(|c| c.as_u64())
                == Some(22) =>
        {
            answers::locations(&l, &[(11, 18)])
        }
        "textDocument/references" => answers::locations(&l, &[(16, 7)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_move_method",
        serde_json::json!({ "path": "src/lib.rs", "line": 10, "character": 12, "to_param": "b", "apply": true }),
    )
    .await
    .expect("it runs");
    let text = text_of(&result);
    assert!(text.contains("`A::sum` is now `B::sum`"), "{text}");
    let now = ws.read("src/lib.rs");
    assert!(
        now.contains("pub struct B {\n    pub m: u32,\n}\n\nimpl B {\n    pub fn sum(&self, a: &A) -> u32 {\n        a.n + self.m + { let b = 1; b }\n    }\n}\n"),
        "{now}"
    );
    assert!(now.contains("    b.sum(&a)\n"), "{now}");
}

/// `code_move_method` needs to know where to: a parameter for a method, a type for an
/// associated function, and exactly one of them.
#[tokio::test]
async fn move_method_asks_for_a_parameter_or_a_type() {
    let ws = workspace();
    write(&ws, "src/lib.rs", MOVE_M);
    commit(&ws);
    let remote = scripted_gateway(Arc::new(move |_, _| serde_json::Value::Null)).await;
    for args in [
        serde_json::json!({ "path": "src/lib.rs", "line": 10, "character": 12 }),
        serde_json::json!({ "path": "src/lib.rs", "line": 10, "character": 12, "to_param": "b", "to_type": "B" }),
    ] {
        let err = execute_tool(remote, &ws.root(), "code_move_method", args)
            .await
            .map(|r| text_of(&r))
            .unwrap_or_else(|e| format!("{e:#}"));
        assert!(
            err.contains("`to_param`") && err.contains("`to_type`"),
            "{err}"
        );
    }
}

/// `code_validate_edits` takes the change as a unified diff or as a WorkspaceEdit too: the diff
/// is applied in memory (a new file created, a deleted one named), and a hunk that fits nowhere
/// is refused by number (#200).
#[tokio::test]
async fn validate_edits_takes_a_diff_or_a_workspace_edit() {
    let ws = workspace();
    let lib = write(&ws, "src/lib.rs", "pub fn a() -> u8 {\n    1\n}\n");
    write(&ws, "src/old.rs", "pub fn o() {}\n");
    commit(&ws);
    let _ = lib;
    // The script plays the analyzer from the text it was sent: `"one"` where a `u8` is due is
    // the only error.
    let texts: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>> = Arc::default();
    let t = Arc::clone(&texts);
    let remote = scripted_gateway(Arc::new(move |method, params| {
        let uri = params
            .pointer("/textDocument/uri")
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string();
        match method {
            "textDocument/didOpen" | "textDocument/didChange" => {
                let text = params
                    .pointer("/textDocument/text")
                    .or_else(|| params.pointer("/contentChanges/0/text"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                t.lock().unwrap().insert(uri, text.to_string());
                serde_json::Value::Null
            }
            "textDocument/diagnostic" => {
                let text = t.lock().unwrap().get(&uri).cloned().unwrap_or_default();
                if text.contains("\"one\"") {
                    answers::error_at(2, 5, "E0308", "mismatched types")
                } else {
                    answers::no_diagnostics()
                }
            }
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,3 @@\n pub fn a() -> u8 {\n-    1\n+    \"one\"\n }\n--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1 @@\n+pub fn n() {}\n--- a/src/old.rs\n+++ /dev/null\n@@ -1 +0,0 @@\n-pub fn o() {}\n";
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({ "diff": diff }),
    )
    .await
    .expect("it runs");
    let text = text_of(&result);
    assert!(
        text.contains("2 file(s) checked together: 1 error(s)"),
        "{text}"
    );
    assert!(text.contains("src/old.rs is deleted by the diff"), "{text}");
    assert_eq!(ws.read("src/lib.rs"), "pub fn a() -> u8 {\n    1\n}\n");

    let stale = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-pub fn b() {}\n+pub fn c() {}\n";
    let err = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({ "diff": stale }),
    )
    .await
    .map(|r| text_of(&r))
    .unwrap_or_else(|e| format!("{e:#}"));
    assert!(err.contains("hunk 1 of src/lib.rs does not apply"), "{err}");

    let uri = format!("file://{}", ws.root().join("src/lib.rs").display());
    let edit = serde_json::json!({ "changes": { uri: [ {
        "range": { "start": { "line": 1, "character": 4 }, "end": { "line": 1, "character": 5 } },
        "newText": "2"
    } ] } });
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_validate_edits",
        serde_json::json!({ "workspace_edit": edit }),
    )
    .await
    .expect("it runs");
    assert!(
        text_of(&result).contains("1 file(s) checked together"),
        "{}",
        text_of(&result)
    );
}

/// A caller the analyzer does not flag and whose name is not `test…` is still a test when an
/// attribute above it says so (`#[tokio::test]`), and `code_impact` lists it (#201).
#[tokio::test]
async fn code_impact_finds_a_test_by_its_attribute() {
    const WITH_CHECK: &str = "pub fn a() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod checks {\n    #[tokio::test]\n    async fn prices() {\n        super::a();\n    }\n}\n";
    let ws = rust_workspace(WITH_CHECK);
    let lib = write(
        &ws,
        "src/lib.rs",
        &WITH_CHECK.replacen("    1\n", "    10\n", 1),
    );
    let uri = format!("file://{}", std::fs::canonicalize(&lib).unwrap().display());
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/documentSymbol" => {
            serde_json::json!([answers::document_symbol("a", 12, 1, 3, 8)])
        }
        "textDocument/prepareCallHierarchy" => serde_json::json!([{
            "name": "a", "kind": 12, "uri": uri,
            "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 2, "character": 1 } },
            "selectionRange": { "start": { "line": 0, "character": 7 }, "end": { "line": 0, "character": 8 } }
        }]),
        "callHierarchy/incomingCalls" => serde_json::json!([{
            "from": {
                "name": "prices", "kind": 12, "uri": uri,
                "range": { "start": { "line": 7, "character": 4 }, "end": { "line": 9, "character": 5 } },
                "selectionRange": { "start": { "line": 7, "character": 13 }, "end": { "line": 7, "character": 19 } }
            },
            "fromRanges": [{ "start": { "line": 8, "character": 15 }, "end": { "line": 8, "character": 16 } }]
        }]),
        _ => serde_json::Value::Null,
    }))
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_impact",
        serde_json::json!({ "depth": 2 }),
    )
    .await
    .expect("the analysis runs");
    let text = text_of(&result);
    assert!(text.contains("1 test(s)"), "{text}");
    assert!(text.contains("• prices  src/lib.rs:8"), "{text}");
    assert!(text.contains("cargo test --workspace -- prices"), "{text}");
}

/// A file written back from a node of another platform is named when its code changed, and
/// not when a formatter only laid it out again: rustfmt cannot depend on the platform (#234).
#[tokio::test]
async fn code_exec_names_a_platform_dependent_edit_but_not_a_reformat() {
    let original =
        "pub fn open() {\n    let mut win = 0u8; let _ = &mut win; // libc::openpty\n}\n";
    let other = if cfg!(target_os = "macos") {
        "linux x86_64"
    } else {
        "macos aarch64"
    };
    let run = |content: &'static str| async move {
        let ws = rust_workspace(original);
        let remote = mock_gateway(Script {
            exec_platform: Some(other.to_string()),
            exec_changes: vec![prod_code_protocol::FileDelta {
                relative_path: "src/lib.rs".to_string(),
                content: Some(content.as_bytes().to_vec()),
                is_executable: false,
            }],
            ..Script::default()
        })
        .await;
        text_of(
            &execute_tool(
                remote,
                &ws.root(),
                "code_exec",
                serde_json::json!({ "argv": ["cargo", "fmt"] }),
            )
            .await
            .expect("exec runs"),
        )
    };
    let formatted =
        run("pub fn open() {\n    let mut win = 0u8;\n    let _ = &mut win; // libc::openpty\n}\n")
            .await;
    assert!(formatted.contains(&format!("({other})")), "{formatted}");
    assert!(!formatted.contains("warning:"), "{formatted}");
    let edited =
        run("pub fn open() {\n    let win = 0u8; let _ = &win; // libc::openpty\n}\n").await;
    assert!(edited.contains("warning: the command ran on"), "{edited}");
    assert!(edited.contains("(src/lib.rs)"), "{edited}");
}

/// `code_lint` with `fix` on a Python project runs ruff's own fix mode on the node and brings
/// the file it rewrote back into the checkout, then lints again (#205).
#[tokio::test]
async fn lint_fix_runs_the_linters_own_fix_mode_for_python() {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"shop\"\nversion = \"0.1.0\"\n",
        ),
        (
            "shop/pricing.py",
            "import os\ndef price(q: int) -> int:\n    return q\n",
        ),
    ]);
    let remote = mock_gateway(Script {
        exec_stdout: b"shop/pricing.py:1:8: F401 [*] `os` imported but unused\n".to_vec(),
        exec_exit: Some(1),
        exec_changes: vec![prod_code_protocol::FileDelta {
            relative_path: "shop/pricing.py".to_string(),
            content: Some(b"def price(q: int) -> int:\n    return q\n".to_vec()),
            is_executable: false,
        }],
        exec_changes_only_for: Some("--fix"),
        ..Script::default()
    })
    .await;
    let result = execute_tool(
        remote,
        &ws.root(),
        "code_lint",
        serde_json::json!({ "fix": true }),
    )
    .await
    .expect("lint runs");
    let text = text_of(&result);
    assert!(text.contains("F401"), "{text}");
    assert!(
        text.contains("fixes: `ruff check . --fix --output-format concise` rewrote 1 file(s)"),
        "{text}"
    );
    assert!(text.contains("  fixed shop/pricing.py\n"), "{text}");
    assert!(text.contains("after the fixes:"), "{text}");
    assert_eq!(
        ws.read("shop/pricing.py"),
        "def price(q: int) -> int:\n    return q\n"
    );
}
