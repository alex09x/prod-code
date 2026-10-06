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
    loop {
        let stream = prod_code_protocol::transport::connect(remote)
            .await
            .with_context(|| format!("Failed to connect to remote gateway at {remote}"))?;
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
            prod_code_mcp::sync::push_workspace_sync(&mut framed, cwd, &identity, None)
                .await
                .context("workspace sync after gateway redirect failed")?;
            prod_code_mcp::watch::mark_synced(cwd, generation);
        }
        return Ok((framed, handshake_resp, remote));
    }
}
