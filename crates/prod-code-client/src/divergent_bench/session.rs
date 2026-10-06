/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{DivergentWorktree, Language, SESSION_SETUP_TIMEOUT, SyncSummary};
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    AnyStream, HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage,
    supported_protocol_versions, validate_selected_protocol_version,
};
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tokio_util::codec::Framed;
use url::Url;

pub(crate) fn locate_symbol(content: &str, symbol: &str) -> Result<(u32, u32)> {
    let mut fallback = None;
    for (idx, line) in content.lines().enumerate() {
        let mut search_from = 0;
        while let Some(rel) = line[search_from..].find(symbol) {
            let col = search_from + rel;
            let before = line[..col].trim_end();
            let after = &line[col + symbol.len()..];
            let is_definition = (before.ends_with("fn") || before.ends_with("func"))
                && after.starts_with(['(', '<']);
            if is_definition {
                return Ok((idx as u32, col as u32));
            }
            fallback.get_or_insert((idx as u32, col as u32));
            search_from = col + symbol.len();
        }
    }
    fallback.ok_or_else(|| anyhow!("symbol `{symbol}` not found in file content"))
}

pub(crate) fn extract_hover_text(result: &serde_json::Value) -> String {
    if let Some(contents) = result.get("contents") {
        if let Some(value) = contents.get("value").and_then(|v| v.as_str()) {
            return value.to_string();
        }
        if let Some(arr) = contents.as_array() {
            return arr
                .iter()
                .filter_map(|item| {
                    item.get("value")
                        .and_then(|v| v.as_str())
                        .or_else(|| item.as_str())
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        if let Some(s) = contents.as_str() {
            return s.to_string();
        }
    }
    result.to_string()
}

pub(crate) async fn read_response_matching_id(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    id: i64,
    query_timeout: Duration,
) -> Result<serde_json::Value> {
    let deadline = Instant::now() + query_timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            bail!("timed out waiting for response id={id}");
        }
        match timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(payload)))) => {
                if let Ok(val) = serde_json::from_str::<serde_json::Value>(&payload)
                    && val.get("id").and_then(|v| v.as_i64()) == Some(id)
                {
                    return Ok(val);
                }
            }
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => bail!("frame decode error: {e}"),
            Ok(None) => bail!("remote gateway closed connection prematurely"),
            Err(_) => bail!("timed out waiting for response id={id}"),
        }
    }
}

/// Sends one full workspace sync (tracked sources and manifests plus current dirty/untracked
/// files) for `root` to the gateway under `workspace_name`, the way `prod-code sync` does on
/// first contact. The persisted watermark is cleared afterwards so no state outlives the run.
pub async fn initial_sync(
    remote: SocketAddr,
    root: &Path,
    workspace_name: &str,
) -> Result<SyncSummary> {
    initial_sync_with_timeout(remote, root, workspace_name, SESSION_SETUP_TIMEOUT).await
}

pub(crate) async fn initial_sync_with_timeout(
    remote: SocketAddr,
    root: &Path,
    workspace_name: &str,
    setup_timeout: Duration,
) -> Result<SyncSummary> {
    timeout(
        setup_timeout,
        initial_sync_unbounded(remote, root, workspace_name),
    )
    .await
    .map_err(|_| {
        anyhow!("timed out after {setup_timeout:?} completing benchmark initial workspace sync")
    })?
}

pub(crate) async fn initial_sync_unbounded(
    remote: SocketAddr,
    root: &Path,
    workspace_name: &str,
) -> Result<SyncSummary> {
    prod_code_mcp::sync::clear_sync_cache(root);
    let identity = prod_code_mcp::sync::WorkspaceIdentity {
        name: workspace_name.to_string(),
        base: workspace_name
            .split_once("--wt-")
            .map(|(base, _)| base.to_string()),
    };
    let start = Instant::now();
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let outcome =
        prod_code_mcp::sync::push_workspace_sync(&mut framed, root, &identity, None).await?;
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "divergent-bench initial sync finished".to_string(),
        })
        .await;
    Ok(SyncSummary {
        workspace_name: workspace_name.to_string(),
        files: outcome.files_updated,
        bytes: outcome.bytes_transferred as u64,
        duration: start.elapsed(),
    })
}

/// Opens a gateway session for `wt`: connect, pre-flight sync, handshake, initialize.
pub(crate) async fn open_session(
    remote: SocketAddr,
    wt: &DivergentWorktree,
    client_name: String,
) -> Result<Framed<AnyStream, ProdCodeCodec>> {
    open_session_with_timeout(remote, wt, client_name, SESSION_SETUP_TIMEOUT).await
}

pub(crate) async fn open_session_with_timeout(
    remote: SocketAddr,
    wt: &DivergentWorktree,
    client_name: String,
    setup_timeout: Duration,
) -> Result<Framed<AnyStream, ProdCodeCodec>> {
    timeout(
        setup_timeout,
        open_session_unbounded(remote, wt, client_name),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "timed out after {setup_timeout:?} completing gateway session setup (connect, pre-flight sync, handshake, initialize)"
        )
    })?
}

pub(crate) async fn open_session_unbounded(
    remote: SocketAddr,
    wt: &DivergentWorktree,
    client_name: String,
) -> Result<Framed<AnyStream, ProdCodeCodec>> {
    let ws_root_str = wt.root.to_string_lossy().to_string();
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());

    let identity = prod_code_mcp::sync::WorkspaceIdentity {
        name: wt.workspace_name.clone(),
        base: None,
    };
    prod_code_mcp::sync::push_workspace_sync(&mut framed, &wt.root, &identity, None)
        .await
        .context("pre-flight workspace sync before gateway handshake")?;

    let supported_versions = supported_protocol_versions();
    framed
        .send(WireMessage::HandshakeRequest(HandshakeRequest {
            protocol_version: PROTOCOL_VERSION,
            supported_versions: Some(supported_versions.clone()),
            capabilities: None,
            client_name,
            client_pid: std::process::id(),
            auth_token: None,
            client_workspace_root: ws_root_str.clone(),
            preferred_engine: None,
            base_workspace_name: Some(wt.workspace_name.clone()),
            engine_subpath: None,
            client_agent: Some(prod_code_protocol::detect_client_agent()),
            client_host: Some(prod_code_protocol::client_host()),
            purpose: None,
            redirect_count: 0,
        }))
        .await
        .context("send gateway handshake request")?;

    let handshake_response = framed
        .next()
        .await
        .transpose()
        .context("wait for gateway handshake response")?
        .ok_or_else(|| {
            anyhow!("remote gateway closed connection waiting for handshake response")
        })?;
    match handshake_response {
        WireMessage::HandshakeResponse(response) => {
            validate_selected_protocol_version(response.protocol_version, &supported_versions)
                .context("gateway returned an incompatible benchmark handshake response")?;
        }
        other => bail!("unexpected handshake response: {other:?}"),
    }

    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "processId": null,
            "rootUri": format!("file://{ws_root_str}"),
            "capabilities": {}
        }
    });
    framed
        .send(WireMessage::LspPayload(init_req.to_string()))
        .await
        .context("send initialize request")?;
    read_response_matching_id(&mut framed, 1, Duration::from_secs(10))
        .await
        .context("wait for initialize response")?;

    let initialized = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {}
    });
    framed
        .send(WireMessage::LspPayload(initialized.to_string()))
        .await
        .context("send initialized notification")?;
    Ok(framed)
}

/// One didOpen/hover/didClose round on an open session; returns the hover text for `wt.symbol`.
pub(crate) async fn hover_in_session(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    wt: &DivergentWorktree,
    language: Language,
    request_id: i64,
) -> Result<String> {
    let timing = std::env::var_os("PROD_CODE_TIMING").is_some();
    let t0 = Instant::now();
    let file_path = &wt.query_file;
    let content = tokio::fs::read_to_string(file_path)
        .await
        .with_context(|| format!("failed to read {file_path:?}"))?;
    let (line, col) = locate_symbol(&content, &wt.symbol)?;
    let file_uri = Url::from_file_path(file_path)
        .map_err(|_| anyhow!("invalid file path for URI: {:?}", file_path))?
        .to_string();
    let t_read = t0.elapsed();

    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": file_uri,
                "languageId": language.label(),
                "version": 1,
                "text": content
            }
        }
    });
    framed
        .send(WireMessage::LspPayload(did_open.to_string()))
        .await?;
    let t_open = t0.elapsed();

    let hover_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "textDocument/hover",
        "params": {
            "textDocument": { "uri": file_uri },
            "position": { "line": line, "character": col }
        }
    });
    framed
        .send(WireMessage::LspPayload(hover_req.to_string()))
        .await?;
    let t_sent = t0.elapsed();
    let response = read_response_matching_id(framed, request_id, Duration::from_secs(30)).await?;
    let t_resp = t0.elapsed();
    if timing {
        eprintln!(
            "[bench-timing] read={:.2}ms did_open_send={:.2}ms hover_send={:.2}ms response_wait={:.2}ms",
            t_read.as_secs_f64() * 1000.0,
            (t_open - t_read).as_secs_f64() * 1000.0,
            (t_sent - t_open).as_secs_f64() * 1000.0,
            (t_resp - t_sent).as_secs_f64() * 1000.0
        );
    }
    let result = response.get("result").unwrap_or(&serde_json::Value::Null);
    if result.is_null() {
        bail!(
            "hover returned null for {} in {}",
            wt.symbol,
            file_path.display()
        );
    }
    let hover_text = extract_hover_text(result);

    let did_close = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didClose",
        "params": { "textDocument": { "uri": file_uri } }
    });
    let _ = framed
        .send(WireMessage::LspPayload(did_close.to_string()))
        .await;
    Ok(hover_text)
}

pub(crate) async fn close_session(mut framed: Framed<AnyStream, ProdCodeCodec>) {
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "divergent-bench session finished".to_string(),
        })
        .await;
}

/// Connect-per-query round: open a session, hover once, close it.
pub(crate) async fn query_once(
    remote: SocketAddr,
    wt: &DivergentWorktree,
    language: Language,
    client_name: String,
) -> Result<String> {
    let mut framed = open_session(remote, wt, client_name).await?;
    let text = hover_in_session(&mut framed, wt, language, 2).await;
    close_session(framed).await;
    text
}
