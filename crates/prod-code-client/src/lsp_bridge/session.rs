/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    HandshakeRequest, PROTOCOL_VERSION, ProdCodeCodec, WireMessage, supported_protocol_versions,
    validate_selected_protocol_version,
};
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;

pub fn resolve_redirect_target(target: &str) -> Result<SocketAddr> {
    prod_code_mcp::cluster::parse_remotes(target)?
        .into_iter()
        .next()
        .context("redirect address resolved to no socket addresses")
}

#[derive(Debug)]
pub(super) struct RedirectTlsRequired;

impl std::fmt::Display for RedirectTlsRequired {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("refusing gateway redirect without server-authenticated TLS; configure a trusted CA or certificate pin")
    }
}

impl std::error::Error for RedirectTlsRequired {}

async fn connect_to_redirect_target(remote: SocketAddr) -> Result<prod_code_protocol::AnyStream> {
    let client_tls = match prod_code_protocol::transport::default_client_tls_built() {
        Some(cached) => cached,
        None => prod_code_protocol::ClientTlsConfig::from_env()?
            .map(prod_code_protocol::ClientTlsConfig::build)
            .transpose()?,
    }
    .ok_or(RedirectTlsRequired)?;
    let token = prod_code_protocol::transport::try_auth_token()?;
    let stream = prod_code_protocol::transport::connect_stream_with_client_config(
        remote,
        token.as_deref(),
        Some(client_tls),
    )
    .await
    .with_context(|| {
        format!("failed to establish authenticated TLS to redirected gateway {remote}")
    })?;
    anyhow::ensure!(
        matches!(&stream, prod_code_protocol::AnyStream::TlsClient(_)),
        "refusing unauthenticated plaintext connection to redirected gateway {remote}"
    );
    Ok(stream)
}

async fn sync_after_handshake(
    remote: SocketAddr,
    cwd: &Path,
    identity: &prod_code_mcp::sync::WorkspaceIdentity,
    redirected: bool,
) -> Result<()> {
    let stream = if redirected {
        connect_to_redirect_target(remote).await?
    } else {
        prod_code_protocol::transport::connect(remote).await?
    };
    let mut sync = Framed::new(stream, ProdCodeCodec::new());
    prod_code_mcp::sync::push_workspace_sync(&mut sync, cwd, identity, None).await?;
    let _ = sync
        .send(WireMessage::Disconnect {
            reason: "sync finished".into(),
        })
        .await;
    Ok(())
}

pub async fn open_editor_session(
    mut remote: SocketAddr,
    engine: Option<&str>,
    cwd: &Path,
    cwd_str: String,
    identity: prod_code_mcp::sync::WorkspaceIdentity,
    initial_redirect_count: u32,
) -> Result<(
    Framed<prod_code_protocol::AnyStream, ProdCodeCodec>,
    prod_code_protocol::HandshakeResponse,
    SocketAddr,
)> {
    let supported_versions = supported_protocol_versions();
    let mut redirect_count = initial_redirect_count;
    let mut refused_by = Vec::new();
    loop {
        let stream = if redirect_count > 0 {
            connect_to_redirect_target(remote).await?
        } else {
            prod_code_protocol::transport::connect(remote)
                .await
                .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?
        };
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        if redirect_count == 0 {
            let generation = prod_code_mcp::watch::current_generation(cwd);
            prod_code_mcp::sync::push_workspace_sync(&mut framed, cwd, &identity, None)
                .await
                .context("workspace sync before the language server session failed")?;
            prod_code_mcp::watch::mark_synced(cwd, generation);
        }
        framed
            .send(WireMessage::HandshakeRequest(HandshakeRequest {
                protocol_version: PROTOCOL_VERSION,
                supported_versions: Some(supported_versions.clone()),
                capabilities: Some(prod_code_protocol::ClientCapabilities {
                    redirects: true,
                    ..Default::default()
                }),
                client_name: "prod-code-client".to_string(),
                client_pid: std::process::id(),
                auth_token: None,
                client_workspace_root: cwd_str.clone(),
                preferred_engine: engine.map(str::to_string),
                base_workspace_name: Some(identity.name.clone()),
                engine_subpath: None,
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
                purpose: Some(prod_code_protocol::PURPOSE_EDITOR.to_string()),
                redirect_count,
                refused_by: refused_by.clone(),
            }))
            .await?;
        let handshake_resp = match framed.next().await {
            Some(Ok(WireMessage::HandshakeResponse(resp))) => resp,
            Some(Ok(WireMessage::Redirect {
                target_addr,
                reason,
            })) => {
                redirect_count += 1;
                if redirect_count > 3 {
                    anyhow::bail!("too many gateway redirects: {reason:?}");
                }
                refused_by.push(remote.to_string());
                tracing::info!(%target_addr, ?reason, "received transparent redirect from gateway");
                remote = resolve_redirect_target(&target_addr)
                    .with_context(|| format!("invalid redirect target address: {target_addr}"))?;
                prod_code_mcp::cluster::remember_placement(&identity.name, remote);
                continue;
            }
            Some(Ok(WireMessage::Disconnect { reason })) => {
                anyhow::bail!("the gateway refused the session: {reason}")
            }
            Some(Ok(other)) => anyhow::bail!("Expected HandshakeResponse, got {:?}", other),
            Some(Err(err)) => return Err(err.into()),
            None => anyhow::bail!("Server closed connection during handshake"),
        };
        validate_selected_protocol_version(handshake_resp.protocol_version, &supported_versions)
            .context("gateway returned an incompatible editor handshake response")?;
        if redirect_count > 0 {
            let generation = prod_code_mcp::watch::current_generation(cwd);
            sync_after_handshake(remote, cwd, &identity, true)
                .await
                .context("workspace sync after gateway redirect failed")?;
            prod_code_mcp::watch::mark_synced(cwd, generation);
        }
        if !handshake_resp.stale_paths.is_empty() {
            let generation = prod_code_mcp::watch::current_generation(cwd);
            prod_code_mcp::sync::resend_lost_files(
                cwd,
                &prod_code_mcp::sync::gateway_node(&framed),
                &handshake_resp.stale_paths,
            );
            sync_after_handshake(remote, cwd, &identity, redirect_count > 0)
                .await
                .context("workspace sync after gateway reported stale files failed")?;
            prod_code_mcp::watch::mark_synced(cwd, generation);
        }
        return Ok((framed, handshake_resp, remote));
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
