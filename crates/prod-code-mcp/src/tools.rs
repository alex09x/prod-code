use crate::protocol::{McpTool, McpToolCallResult};
use crate::sync::scan_workspace_files;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, SyncRequest, WireMessage,
};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use url::Url;

/// Return list of tools exposed by the MCP server.
pub fn list_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "code_definition".to_string(),
            description: "Find symbol definition (function, struct, type, variable, module) at specified file and 1-based line/column position."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_references".to_string(),
            description: "Find all reference locations of a symbol across the workspace at specified file and 1-based line/column position."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    },
                    "include_declarations": {
                        "type": "boolean",
                        "description": "Include the declaration site in the reference list (default: false)"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_outline".to_string(),
            description: "Extract the structural symbol outline (functions, structs, enums, traits, classes) with line numbers from a source file."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum hierarchy depth (default: 3)"
                    }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_hover".to_string(),
            description: "Inspect the type signature, docstring, and documentation for a symbol at specified file and line/column position."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_type_at".to_string(),
            description: "Alias for code_hover: inspect the type and documentation of a code expression."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_status".to_string(),
            description: "Check remote prod-code gateway health, memory RSS, active engines, loaded workspaces, and network RTT."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "code_sync".to_string(),
            description: "Push local modified workspace files to the remote server over 10G LAN for immediate compilation and analysis."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional specific subfolder or file to sync. Defaults to entire workspace root."
                    }
                }
            }),
        },
    ]
}

/// Execute an MCP tool call against the remote gateway.
pub async fn execute_tool(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    match tool_name {
        "code_definition" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;

            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/definition",
                params,
            )
            .await?;

            let mut out = String::new();
            if let Some(arr) = res.as_array() {
                if arr.is_empty() {
                    out.push_str("No definition found.");
                } else {
                    for (i, loc) in arr.iter().enumerate() {
                        let uri = loc
                            .get("uri")
                            .or_else(|| loc.get("targetUri"))
                            .and_then(|u| u.as_str())
                            .unwrap_or("");
                        let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                        let start_line = range
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("line"))
                            .and_then(|l| l.as_u64())
                            .unwrap_or(0)
                            + 1;
                        let start_col = range
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("character"))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0)
                            + 1;
                        if i > 0 {
                            out.push('\n');
                        }
                        out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
                    }
                }
            } else if let Some(obj) = res.as_object() {
                let uri = obj.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let start_line = obj
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = obj
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
            } else {
                out.push_str("No definition found.");
            }

            Ok(McpToolCallResult::text(out))
        }

        "code_references" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;
            let include_decl = args
                .get("include_declarations")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
                "context": { "includeDeclaration": include_decl }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/references",
                params,
            )
            .await?;

            let mut out = String::new();
            if let Some(arr) = res.as_array() {
                if arr.is_empty() {
                    out.push_str("No references found.");
                } else {
                    out.push_str(&format!("Found {} reference(s):\n", arr.len()));
                    for loc in arr {
                        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                        let start_line = loc
                            .get("range")
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("line"))
                            .and_then(|l| l.as_u64())
                            .unwrap_or(0)
                            + 1;
                        let start_col = loc
                            .get("range")
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("character"))
                            .and_then(|c| c.as_u64())
                            .unwrap_or(0)
                            + 1;
                        out.push_str(&format!("  • {uri}:{start_line}:{start_col}\n"));
                    }
                }
            } else {
                out.push_str("No references found.");
            }

            Ok(McpToolCallResult::text(out.trim_end()))
        }

        "code_outline" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/documentSymbol",
                params,
            )
            .await?;

            let mut out = String::new();
            if let Some(arr) = res.as_array() {
                out.push_str(&format!("Outline for {path_str}:\n"));
                for sym in arr {
                    let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
                    let kind_str = match kind {
                        5 => "Class/Struct",
                        6 => "Method",
                        11 => "Function",
                        12 => "Variable",
                        13 => "Constant",
                        23 => "Struct",
                        _ => "Symbol",
                    };
                    let line = sym
                        .get("range")
                        .or_else(|| sym.get("location").and_then(|l| l.get("range")))
                        .and_then(|r| r.get("start"))
                        .and_then(|s| s.get("line"))
                        .and_then(|l| l.as_u64())
                        .unwrap_or(0)
                        + 1;
                    out.push_str(&format!("  [{kind_str}] {name} (line {line})\n"));
                }
            } else {
                out.push_str("No outline symbols available.");
            }

            Ok(McpToolCallResult::text(out.trim_end()))
        }

        "code_hover" | "code_type_at" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let line = args
                .get("line")
                .and_then(|v| v.as_u64())
                .context("Missing 'line' argument")? as u32;
            let character = args
                .get("character")
                .and_then(|v| v.as_u64())
                .context("Missing 'character' argument")? as u32;

            let file_path = resolve_file_path(workspace_root, path_str);
            let file_uri = Url::from_file_path(&file_path)
                .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
                .to_string();

            let params = serde_json::json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
            });

            let res = execute_lsp_query(
                remote,
                workspace_root,
                &file_path,
                "textDocument/hover",
                params,
            )
            .await?;

            if let Some(contents) = res.get("contents") {
                if let Some(val) = contents.get("value").and_then(|v| v.as_str()) {
                    return Ok(McpToolCallResult::text(val));
                } else if let Some(arr) = contents.as_array() {
                    let text = arr
                        .iter()
                        .filter_map(|i| i.get("value").and_then(|v| v.as_str()))
                        .collect::<Vec<_>>()
                        .join("\n\n");
                    return Ok(McpToolCallResult::text(text));
                }
            }

            Ok(McpToolCallResult::text("No hover information available."))
        }

        "code_status" => {
            let start = std::time::Instant::now();
            let stream = TcpStream::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
            let rtt = start.elapsed();

            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            framed.send(WireMessage::StatusRequest).await?;

            if let Some(msg_res) = framed.next().await {
                match msg_res? {
                    WireMessage::StatusResponse(resp) => {
                        let hours = resp.uptime_seconds / 3600;
                        let minutes = (resp.uptime_seconds % 3600) / 60;
                        let seconds = resp.uptime_seconds % 60;
                        let mem = resp.memory_rss_mb().unwrap_or(0.0);

                        let info = format!(
                            "⚡ prod-code Gateway Status\n\
                             • Address: {remote} ({rtt:.2?} RTT)\n\
                             • Server PID: {}\n\
                             • Uptime: {hours}h {minutes}m {seconds}s\n\
                             • Memory RSS: {mem:.2} MB\n\
                             • Active Sessions: {}\n\
                             • Loaded Workspaces: {}\n\
                             • Queries Handled: {} (in-flight: {})\n\
                             • Engines: {}\n\
                             • Status: HEALTHY",
                            resp.server_pid,
                            resp.active_sessions,
                            resp.loaded_workspaces,
                            resp.total_queries,
                            resp.active_queries,
                            resp.detected_engines.join(", ")
                        );
                        Ok(McpToolCallResult::text(info))
                    }
                    other => Ok(McpToolCallResult::error(format!(
                        "Unexpected response from gateway: {other:?}"
                    ))),
                }
            } else {
                Ok(McpToolCallResult::error(
                    "Gateway closed connection without status response",
                ))
            }
        }

        "code_sync" => {
            let subpath = args.get("path").and_then(|v| v.as_str()).map(Path::new);
            let deltas = scan_workspace_files(workspace_root, subpath)?;
            let file_count = deltas.len();

            let stream = TcpStream::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            let req = SyncRequest {
                client_workspace_root: workspace_root.to_string_lossy().to_string(),
                files: deltas,
                clean_others: false,
                base_workspace_name: None,
            };

            framed.send(WireMessage::SyncRequest(req)).await?;

            if let Some(msg_res) = framed.next().await {
                match msg_res? {
                    WireMessage::SyncResponse(resp) => {
                        let kb = (resp.bytes_transferred as f64) / 1024.0;
                        let info = format!(
                            "⚡ Fast-Sync Completed in {}ms\n\
                             • Files scanned: {file_count}\n\
                             • Files updated: {}\n\
                             • Files deleted: {}\n\
                             • Data transferred: {kb:.1} KB\n\
                             • Remote workspace: {}",
                            resp.duration_ms,
                            resp.files_updated,
                            resp.files_deleted,
                            resp.server_workspace_root
                        );
                        Ok(McpToolCallResult::text(info))
                    }
                    other => Ok(McpToolCallResult::error(format!(
                        "Unexpected response: {other:?}"
                    ))),
                }
            } else {
                Ok(McpToolCallResult::error(
                    "Gateway closed connection without sync response",
                ))
            }
        }

        unknown => Ok(McpToolCallResult::error(format!("Unknown tool: {unknown}"))),
    }
}

fn resolve_file_path(workspace_root: &Path, path_str: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(path_str);
    if p.is_absolute() {
        p
    } else {
        workspace_root.join(p)
    }
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
async fn execute_lsp_query(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let root_str = workspace_root.to_string_lossy().to_string();
    let workspace_name = crate::sync::workspace_identity(workspace_root).name;
    let file_uri = Url::from_file_path(file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();

    let file_content = tokio::fs::read_to_string(file_path)
        .await
        .unwrap_or_default();

    let mut framed = {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let stream = TcpStream::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());

            // 1a. Transparent pre-flight sync before the handshake: manifest probe on first contact
            // (seeded from the origin repository's copy), watermark delta afterwards.
            let identity = crate::sync::workspace_identity(workspace_root);
            if let Err(e) =
                crate::sync::push_workspace_sync(&mut framed, workspace_root, &identity, None).await
            {
                tracing::warn!(error = %e, "pre-flight workspace sync failed");
            }

            // 1. Handshake
            framed
                .send(WireMessage::HandshakeRequest(HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    client_name: "prod-code-mcp".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: root_str.clone(),
                    preferred_engine: None,
                    base_workspace_name: Some(workspace_name.clone()),
                }))
                .await?;

            let handshake = match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
                other => anyhow::bail!("Unexpected handshake response: {:?}", other),
            };

            // Self-heal: the gateway keyed this workspace on an empty or reset directory (its
            // detected engine does not match our manifest) while our watermark still claims
            // everything was sent. Forget the watermark, push the full tree and reconnect;
            // the gateway reloads a workspace whose engine kind changed.
            if attempt == 1
                && let Some(expected) = crate::sync::expected_engine(workspace_root)
                && handshake.detected_engine != expected
            {
                crate::sync::clear_sync_cache(workspace_root);
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "engine mismatch; resyncing workspace".to_string(),
                    })
                    .await;
                continue;
            }
            break framed;
        }
    };

    // 2. LSP Initialize
    let folder_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{root_str}"),
            "workspaceFolders": [
                {
                    "name": folder_name,
                    "uri": format!("file://{root_str}")
                }
            ],
            "capabilities": {
                "workspace": {
                    "workspaceFolders": true,
                    "configuration": true
                },
                "textDocument": {
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"]
                    },
                    "definition": {
                        "linkSupport": true
                    },
                    "documentSymbol": {
                        "hierarchicalDocumentSymbolSupport": true
                    },
                    "references": {}
                }
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(init_req.to_string()))
        .await?;

    // Await init response (matching id: 1)
    let init_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < init_deadline {
        let remaining = init_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if serde_json::from_str::<serde_json::Value>(&resp_json)
                    .ok()
                    .and_then(|val| val.get("id").and_then(|id| id.as_i64()))
                    == Some(1)
                {
                    break;
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => anyhow::bail!("Frame decode error during initialize: {}", e),
            Ok(None) => anyhow::bail!("Server closed connection during initialize"),
            Err(_) => anyhow::bail!("Timeout waiting for initialize response"),
        }
    }

    // 3. LSP Initialized notification
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await?;

    // 4. LSP didOpen notification
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri.clone(),
                "languageId": "rust",
                "version": 1,
                "text": file_content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;

    // 5. Send targeted query with id = 2
    let query_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": method,
        "params": params
    });
    framed
        .send(WireMessage::LspPayload(query_req.to_string()))
        .await?;

    // 6. Read response matching id = 2
    let query_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
    while tokio::time::Instant::now() < query_deadline {
        let remaining = query_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_json)
                    .map_err(|_| ())
                    .and_then(|v| {
                        if v.get("id").and_then(|id| id.as_i64()) == Some(2) {
                            Ok(v)
                        } else {
                            Err(())
                        }
                    })
                {
                    let did_close = serde_json::json!({
                        "jsonrpc": "2.0",
                        "method": "textDocument/didClose",
                        "params": {
                            "textDocument": {
                                "uri": file_uri
                            }
                        }
                    });
                    let _ = framed
                        .send(WireMessage::LspPayload(did_close.to_string()))
                        .await;
                    let _ = framed
                        .send(WireMessage::Disconnect {
                            reason: "query finished".to_string(),
                        })
                        .await;
                    return Ok(val
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null));
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => anyhow::bail!("Frame decode error: {}", e),
            Ok(None) => anyhow::bail!("Remote closed connection prematurely"),
            Err(_) => anyhow::bail!("Timeout waiting for LSP response to {}", method),
        }
    }

    anyhow::bail!("No response received for query {}", method)
}
