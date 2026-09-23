//! Native Model Context Protocol (MCP) server for prod-code AI agent fleets.

pub mod cluster;
pub mod compile_check;
pub mod dead_code;
pub mod diagnostics;
pub mod dossier;
pub mod encapsulate_field;
pub mod exec;
pub mod extract_delegate;
pub mod extract_field;
pub mod extract_parameter;
pub mod extract_trait;
pub mod fixit;
pub mod fixture;
pub mod generify;
pub mod hot_reload;
pub mod impact;
pub mod inline_parameter;
pub mod introduce_variable;
pub mod invert_boolean;
pub mod invert_value;
pub mod lang;
pub mod loop_to_iterator;
pub mod make_static;
pub mod move_item;
pub mod parameter_object;
pub mod protocol;
pub mod prune;
pub mod refactor;
pub mod remote_fs;
pub mod rename_accessors;
pub mod schema;
pub mod search;
pub mod session;
pub mod shadow;
pub mod signature;
pub mod slice;
pub mod sync;
pub mod to_method;
pub mod tools;
pub mod type_migration;
pub mod verify;
pub mod watch;
pub mod wrap_return;

pub use protocol::{MCP_PROTOCOL_VERSION, SERVER_NAME, SERVER_VERSION};
pub use sync::scan_workspace_files;
pub use tools::{execute_tool, list_tools};

use anyhow::{Context, Result};
use protocol::{JsonRpcRequest, JsonRpcResponse};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Process a single incoming MCP JSON-RPC message.
/// Returns Ok(Some(response)) for requests that require a response, or Ok(None) for notifications.
pub async fn handle_mcp_request(
    remote: SocketAddr,
    workspace_root: &Path,
    req_val: serde_json::Value,
) -> Result<Option<serde_json::Value>> {
    let req: JsonRpcRequest = match serde_json::from_value(req_val) {
        Ok(r) => r,
        Err(e) => {
            let err_resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
            return Ok(Some(serde_json::to_value(err_resp)?));
        }
    };

    let id = req.id.clone();

    match req.method.as_str() {
        "initialize" => {
            let resp = JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "protocolVersion": MCP_PROTOCOL_VERSION,
                    "capabilities": {
                        "tools": { "listChanged": true }
                    },
                    "serverInfo": {
                        "name": SERVER_NAME,
                        "version": SERVER_VERSION
                    },
                    "instructions": crate::protocol::AGENT_INSTRUCTIONS
                }),
            );
            Ok(Some(serde_json::to_value(resp)?))
        }

        "notifications/initialized" | "initialized" => {
            // Notification: no response required
            Ok(None)
        }

        "ping" => {
            let resp = JsonRpcResponse::success(id, serde_json::json!({}));
            Ok(Some(serde_json::to_value(resp)?))
        }

        "tools/list" => {
            let tools = list_tools();
            let resp = JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "tools": tools
                }),
            );
            Ok(Some(serde_json::to_value(resp)?))
        }

        "tools/call" => {
            let params = req.params.unwrap_or(serde_json::json!({}));
            let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::json!({}));

            match execute_tool(remote, workspace_root, tool_name, arguments).await {
                Ok(call_result) => {
                    let resp = JsonRpcResponse::success(id, serde_json::to_value(call_result)?);
                    Ok(Some(serde_json::to_value(resp)?))
                }
                Err(e) => {
                    let call_result = protocol::McpToolCallResult::error(e.to_string());
                    let resp = JsonRpcResponse::success(id, serde_json::to_value(call_result)?);
                    Ok(Some(serde_json::to_value(resp)?))
                }
            }
        }

        unknown => {
            // If it's a notification without an ID, do not return an error
            if id.is_none() {
                Ok(None)
            } else {
                let resp =
                    JsonRpcResponse::error(id, -32601, format!("Method not found: {unknown}"));
                Ok(Some(serde_json::to_value(resp)?))
            }
        }
    }
}

/// Run full stdio MCP server loop.
pub async fn run_stdio_mcp_server(remote: SocketAddr, workspace_root: PathBuf) -> Result<()> {
    let resumed = std::env::var_os(hot_reload::RESUMED_ENV).is_some();
    tracing::info!(
        server = SERVER_NAME,
        version = SERVER_VERSION,
        gateway = %remote,
        workspace = %workspace_root.display(),
        resumed,
        "Starting prod-code stdio MCP server"
    );

    // Hot reload: watch our own executable and swap to a newly installed one between
    // requests (see `hot_reload`).
    let exe = std::env::current_exe().ok();
    serve_mcp_requests(
        remote,
        workspace_root,
        exe,
        resumed,
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}

/// The request/response loop, generic over its transport so it can be driven by a test without
/// touching the process's real stdio. `run_stdio_mcp_server` is a thin wrapper around this with
/// the real standard streams; `exe` is `None` there only when the running binary's own path
/// could not be resolved, in which case hot reload is simply not offered.
async fn serve_mcp_requests<R, W>(
    remote: SocketAddr,
    workspace_root: PathBuf,
    exe: Option<PathBuf>,
    resumed: bool,
    mut reader: BufReader<R>,
    mut writer: W,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let reload_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reload_notify = std::sync::Arc::new(tokio::sync::Notify::new());
    if let Some(exe) = &exe
        && let Some(initial) = hot_reload::stamp(exe)
    {
        hot_reload::spawn_watch(
            exe.clone(),
            initial,
            std::sync::Arc::clone(&reload_flag),
            std::sync::Arc::clone(&reload_notify),
        );
    }
    if resumed {
        // The session was initialised with the previous binary: tell the client to refresh
        // its tool list from this one.
        writer
            .write_all(hot_reload::tools_list_changed().as_bytes())
            .await?;
        writer.flush().await?;
    }

    let mut pending: Vec<u8> = Vec::new();
    loop {
        if reload_flag.load(std::sync::atomic::Ordering::Acquire)
            && pending.is_empty()
            && reader.buffer().is_empty()
            && let Some(exe) = &exe
        {
            writer
                .write_all(hot_reload::tools_list_changed().as_bytes())
                .await?;
            writer.flush().await?;
            tracing::info!(exe = %exe.display(), "re-executing the installed binary");
            let err = hot_reload::reexec(exe);
            tracing::error!(error = %err, "hot reload failed; continuing with the current binary");
            reload_flag.store(false, std::sync::atomic::Ordering::Release);
        }

        let consumed = tokio::select! {
            filled = reader.fill_buf() => {
                let buf = filled.context("Failed reading from stdin")?;
                if buf.is_empty() {
                    break; // EOF
                }
                pending.extend_from_slice(buf);
                buf.len()
            }
            _ = reload_notify.notified() => 0,
        };
        reader.consume(consumed);

        while let Some(line) = hot_reload::take_line(&mut pending) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(req_json) = serde_json::from_str::<serde_json::Value>(trimmed) {
                match handle_mcp_request(remote, &workspace_root, req_json).await {
                    Ok(Some(resp_val)) => {
                        let mut out = serde_json::to_string(&resp_val)?;
                        out.push('\n');
                        writer.write_all(out.as_bytes()).await?;
                        writer.flush().await?;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::error!(error = %e, "MCP request handler internal error");
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn test_mcp_initialize() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {}
            }
        });

        let resp_opt = handle_mcp_request(dummy_addr, &root, req).await.unwrap();
        assert!(resp_opt.is_some());
        let resp = resp_opt.unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["serverInfo"]["name"], "prod-code-mcp");
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
    }

    #[tokio::test]
    async fn test_mcp_tools_list() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        });

        let resp_opt = handle_mcp_request(dummy_addr, &root, req).await.unwrap();
        assert!(resp_opt.is_some());
        let resp = resp_opt.unwrap();
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array().unwrap();
        let tool_names: Vec<_> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();

        assert!(tool_names.contains(&"code_definition"));
        assert!(tool_names.contains(&"code_references"));
        assert!(tool_names.contains(&"code_outline"));
        assert!(tool_names.contains(&"code_hover"));
        assert!(tool_names.contains(&"code_status"));
        assert!(tool_names.contains(&"code_sync"));
    }

    #[tokio::test]
    async fn test_mcp_ping() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "ping"
        });

        let resp = handle_mcp_request(dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 3);
        assert_eq!(resp["result"], serde_json::json!({}));
    }

    #[tokio::test]
    async fn test_mcp_unknown_method() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "foo/bar"
        });

        let resp = handle_mcp_request(dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 4);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn test_mcp_unknown_notification_gets_no_response() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({ "jsonrpc": "2.0", "method": "foo/bar" });

        let resp = handle_mcp_request(dummy_addr, &root, req).await.unwrap();
        assert!(resp.is_none(), "a notification gets no reply: {resp:?}");
    }

    #[tokio::test]
    async fn test_mcp_initialized_notification_gets_no_response() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        for method in ["notifications/initialized", "initialized"] {
            let req = serde_json::json!({ "jsonrpc": "2.0", "method": method });
            let resp = handle_mcp_request(dummy_addr, &root, req).await.unwrap();
            assert!(resp.is_none(), "{method}: {resp:?}");
        }
    }

    #[tokio::test]
    async fn test_mcp_malformed_request_is_a_parse_error() {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");

        // Missing the required `method` field: the request itself does not deserialize.
        let resp = handle_mcp_request(dummy_addr, &root, serde_json::json!({}))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["error"]["code"], -32700);
        assert!(
            resp["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("Parse error:"),
            "{resp:?}"
        );
    }

    #[tokio::test]
    async fn test_mcp_tools_call_reports_a_failed_tool_as_a_normal_response() {
        // A closed port: the tool's own connection attempt fails, and that failure is
        // reported as a normal (non-transport) JSON-RPC response, not a handler error. Port 1,
        // not a port bound and dropped: with the whole suite running in parallel, a freed
        // ephemeral port is taken by another test's listener often enough that the connection
        // succeeds and is reset instead of refused.
        let dummy_addr: std::net::SocketAddr = "127.0.0.1:1".parse().unwrap();
        let root = PathBuf::from("/tmp");

        let req = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "code_status", "arguments": {} }
        });

        let resp = handle_mcp_request(dummy_addr, &root, req)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resp["id"], 5);
        assert_eq!(resp["result"]["isError"], true);
        let text = resp["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Failed to connect"), "{text}");
    }

    /// Wires `serve_mcp_requests` to an in-memory duplex stream: the returned handle is the
    /// client's end (write requests into it, read responses back out of it), and the join
    /// handle resolves once the client end (or its clone) is dropped, which the server reads
    /// as EOF.
    fn spawn_server(
        resumed: bool,
    ) -> (tokio::io::DuplexStream, tokio::task::JoinHandle<Result<()>>) {
        let dummy_addr: SocketAddr = "127.0.0.1:9400".parse().unwrap();
        let root = PathBuf::from("/tmp");
        let (client, server) = tokio::io::duplex(64 * 1024);
        let (server_read, server_write) = tokio::io::split(server);
        let handle = tokio::spawn(serve_mcp_requests(
            dummy_addr,
            root,
            None, // no real executable to hot-reload from in a test
            resumed,
            BufReader::new(server_read),
            server_write,
        ));
        (client, handle)
    }

    #[tokio::test]
    async fn serve_mcp_requests_answers_a_request_then_stops_at_eof() {
        let (mut client, handle) = spawn_server(false);
        client
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n")
            .await
            .unwrap();

        let mut buf = [0u8; 4096];
        let n = client.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        let resp: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"], serde_json::json!({}));

        drop(client); // EOF: the server's read half sees a closed connection
        let result = handle.await.unwrap();
        assert!(result.is_ok(), "{result:?}");
    }

    #[tokio::test]
    async fn serve_mcp_requests_skips_blank_lines_and_invalid_json() {
        let (mut client, handle) = spawn_server(false);
        client
            .write_all(
                b"\n   \nnot json at all\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n",
            )
            .await
            .unwrap();

        let mut buf = [0u8; 4096];
        let n = client.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        // Exactly one response line: the blank lines and the invalid JSON produced nothing.
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        let resp: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(resp["id"], 2);

        drop(client);
        assert!(handle.await.unwrap().is_ok());
    }

    #[tokio::test]
    async fn serve_mcp_requests_announces_a_resumed_session_before_any_request() {
        let (mut client, handle) = spawn_server(true);

        let mut buf = [0u8; 4096];
        let n = client.read(&mut buf).await.unwrap();
        let line = String::from_utf8_lossy(&buf[..n]);
        assert_eq!(line, hot_reload::tools_list_changed());

        drop(client);
        assert!(handle.await.unwrap().is_ok());
    }
}
