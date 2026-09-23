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
                if !script.exec_stdout.is_empty() {
                    framed
                        .send(WireMessage::ExecChunk(ExecChunk {
                            stderr: false,
                            data: Some(script.exec_stdout.clone()),
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
                framed
                    .send(WireMessage::ExecExit(ExecExit {
                        exit_code: script.exec_exit,
                        duration_ms: 5,
                        server_workspace_root: req.client_workspace_root,
                        timed_out: false,
                        error: None,
                        usage: None,
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

const MOVE_M: &str = "pub struct A {\n    pub n: u32,\n}\n\npub struct B {\n    pub m: u32,\n}\n\nimpl A {\n    pub fn sum(&self, b: &B) -> u32 {\n        self.n + b.m\n    }\n}\n\npub fn f(a: &A, b: &B) -> u32 {\n    a.sum(b)\n}\n";

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
    let remote = scripted_gateway(Arc::new(move |method, _| match method {
        "textDocument/definition" => answers::locations(&l, &[(5, 12)]),
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
        now.contains("pub struct B {\n    pub m: u32,\n}\n\nimpl B {\n    pub fn sum(&self, a: &A) -> u32 {\n        a.n + self.m\n    }\n}\n"),
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
