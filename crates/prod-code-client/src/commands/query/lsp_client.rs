/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::lsp_bridge::resolve_redirect_target;
use crate::timing::QueryTiming;
use crate::workspace::{detect_workspace_name, find_workspace_root};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage, supported_protocol_versions,
    validate_selected_protocol_version,
};
use std::env;
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;
use url::Url;

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
pub async fn execute_lsp_query(
    mut remote: SocketAddr,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let cwd = env::current_dir().context("Failed to determine current working directory")?;

    let abs_path = if file_path.is_absolute() {
        std::fs::canonicalize(file_path).unwrap_or_else(|_| file_path.to_path_buf())
    } else {
        let joined = cwd.join(file_path);
        std::fs::canonicalize(&joined).unwrap_or(joined)
    };

    let ws_root = find_workspace_root(&abs_path).unwrap_or_else(|| cwd.clone());
    let ws_root_str = ws_root.to_string_lossy().to_string();
    let base_ws_name = detect_workspace_name(&ws_root);

    let file_uri = Url::from_file_path(&abs_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", abs_path))?
        .to_string();

    let file_content = tokio::fs::read_to_string(&abs_path)
        .await
        .with_context(|| format!("Failed to read file {:?}", abs_path))?;

    let mut timing = QueryTiming::new();
    let mut framed = {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let stream = prod_code_protocol::transport::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            timing.mark("connect");

            let identity = prod_code_mcp::sync::workspace_identity(&ws_root);
            if let Err(e) =
                prod_code_mcp::sync::push_workspace_sync(&mut framed, &ws_root, &identity, None)
                    .await
            {
                tracing::warn!(error = %e, "pre-flight workspace sync failed");
            }
            timing.mark("preflight_sync");

            let (engine_subpath, expected_engine) =
                prod_code_mcp::sync::engine_project(&ws_root, &abs_path);
            let supported_versions = supported_protocol_versions();
            framed
                .send(WireMessage::HandshakeRequest(HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    supported_versions: Some(supported_versions.clone()),
                    capabilities: Some(prod_code_protocol::ClientCapabilities {
                        redirects: true,
                        ..Default::default()
                    }),
                    client_name: "prod-code-cli".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: ws_root_str.clone(),
                    preferred_engine: engine_subpath
                        .as_ref()
                        .and(expected_engine)
                        .map(str::to_string),
                    base_workspace_name: base_ws_name.clone(),
                    engine_subpath,
                    client_agent: Some(prod_code_protocol::detect_client_agent()),
                    client_host: Some(prod_code_protocol::client_host()),
                    purpose: None,
                    redirect_count: (attempt - 1) as u32,
                }))
                .await?;

            let handshake = match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
                Some(Ok(WireMessage::Redirect {
                    target_addr,
                    reason,
                })) => {
                    if attempt > 3 {
                        anyhow::bail!("too many gateway redirects: {reason:?}");
                    }
                    tracing::info!(%target_addr, ?reason, "received transparent redirect from gateway");
                    remote = resolve_redirect_target(&target_addr).with_context(|| {
                        format!("invalid redirect target address: {target_addr}")
                    })?;
                    prod_code_mcp::cluster::remember_placement(&identity.name, remote);
                    continue;
                }
                other => anyhow::bail!("Unexpected handshake response: {:?}", other),
            };
            validate_selected_protocol_version(handshake.protocol_version, &supported_versions)
                .context("gateway returned an incompatible handshake response")?;

            if attempt == 1
                && let Some(expected) = expected_engine
                && handshake.detected_engine != expected
            {
                prod_code_mcp::sync::clear_sync_cache(&ws_root);
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "engine mismatch; resyncing workspace".to_string(),
                    })
                    .await;
                continue;
            }
            prod_code_mcp::sync::resend_lost_files(
                &ws_root,
                &prod_code_mcp::sync::gateway_node(&framed),
                &handshake.stale_paths,
            );
            timing.mark("handshake");
            break framed;
        }
    };

    let folder_name = ws_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace");
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{}", ws_root.to_string_lossy()),
            "workspaceFolders": [
                {
                    "name": folder_name,
                    "uri": format!("file://{}", ws_root.to_string_lossy())
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

    let init_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
    while tokio::time::Instant::now() < init_deadline {
        let remaining = init_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if serde_json::from_str::<serde_json::Value>(&resp_json)
                    .ok()
                    .filter(|val| val.get("method").is_none())
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

    timing.mark("initialize");
    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await?;

    let language_id = prod_code_mcp::lang::language_id_for_path(&abs_path);
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": language_id,
                "version": 1,
                "text": file_content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;
    timing.mark("did_open_sent");

    let query_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": method,
        "params": params
    });
    framed
        .send(WireMessage::LspPayload(query_req.to_string()))
        .await?;

    let query_deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(60);
    while tokio::time::Instant::now() < query_deadline {
        let remaining = query_deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(resp_json)))) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&resp_json)
                    .map_err(|_| ())
                    .and_then(|v| {
                        if v.get("method").is_none()
                            && v.get("id").and_then(|id| id.as_i64()) == Some(2)
                        {
                            Ok(v)
                        } else {
                            Err(())
                        }
                    })
                {
                    timing.mark("query_response");
                    timing.report();
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
                    if let Some(err) = val.get("error") {
                        let message = err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown error");
                        anyhow::bail!("{method} failed: {message}");
                    }
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
