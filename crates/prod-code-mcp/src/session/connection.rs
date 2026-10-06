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
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;

use crate::sync::{
    WorkspaceIdentity, engine_project, gateway_node, push_workspace_sync, resend_lost_files,
    workspace_identity,
};

use super::types::{LspSession, OPEN_BUDGET, timeout_error};

impl LspSession {
    /// Opens a session on `remote` for the checkout at `root`. `hint` selects a nested
    /// project (any path inside it); the root project otherwise.
    pub async fn open(remote: SocketAddr, root: &Path, hint: Option<&Path>) -> Result<Self> {
        Self::open_with_purpose(remote, root, hint, None).await
    }

    /// A session that opens proposed texts only to validate them. The gateway serves it from a
    /// second engine for the workspace, so an overlay that changes what a widely imported file
    /// declares — and the revert when the session closes — never invalidates the main engine's
    /// work, and the next ordinary query does not pay for it (#73).
    pub async fn open_for_validation(
        remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
    ) -> Result<Self> {
        Self::open_with_purpose(
            remote,
            root,
            hint,
            Some(prod_code_protocol::PURPOSE_VALIDATION),
        )
        .await
    }

    pub(crate) async fn open_with_purpose(
        remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
        purpose: Option<&str>,
    ) -> Result<Self> {
        Self::open_with_budget(remote, root, hint, purpose, OPEN_BUDGET).await
    }

    pub(crate) async fn open_with_budget(
        remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
        purpose: Option<&str>,
        budget: std::time::Duration,
    ) -> Result<Self> {
        tokio::time::timeout(budget, Self::open_inner(remote, root, hint, purpose))
            .await
            .map_err(|_| {
                timeout_error("opening the session (connect, sync or engine load)", budget)
            })?
    }

    async fn open_inner(
        mut remote: SocketAddr,
        root: &Path,
        hint: Option<&Path>,
        purpose: Option<&str>,
    ) -> Result<Self> {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let root_str = root.to_string_lossy().to_string();
        let identity: WorkspaceIdentity = workspace_identity(&root);
        let mut redirect_count = 0;
        let (framed, handshake) = loop {
            let stream = prod_code_protocol::transport::connect(remote)
                .await
                .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
            let mut framed = Framed::new(stream, ProdCodeCodec::new());
            push_workspace_sync(&mut framed, &root, &identity, None)
                .await
                .context("pre-flight workspace sync failed")?;
            let (engine_subpath, mut engine) = engine_project(&root, hint.unwrap_or(&root));
            if let Some(h) = hint
                && let Some(own) = crate::sync::engine_for_file(h)
                && engine == crate::sync::expected_engine(&root)
                && Some(own) != engine
            {
                engine = Some(own);
            }
            let preferred_engine = engine.map(str::to_string);
            let supported_versions = supported_protocol_versions();
            framed
                .send(WireMessage::HandshakeRequest(HandshakeRequest {
                    protocol_version: PROTOCOL_VERSION,
                    supported_versions: Some(supported_versions.clone()),
                    capabilities: Some(prod_code_protocol::ClientCapabilities {
                        direct_edit: true,
                        watch_files: true,
                        indexing_status: true,
                        shadow_runs: true,
                        multi_root: true,
                        sync_chunking: false,
                        unix_socket_local: cfg!(unix),
                        redirects: true,
                    }),
                    client_name: "prod-code-batch".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: root_str.clone(),
                    preferred_engine,
                    base_workspace_name: Some(identity.name.clone()),
                    engine_subpath,
                    client_agent: Some(prod_code_protocol::detect_client_agent()),
                    client_host: Some(prod_code_protocol::client_host()),
                    purpose: purpose.map(str::to_string),
                    redirect_count,
                }))
                .await?;
            let handshake = match framed.next().await {
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
                    remote = crate::cluster::parse_remotes(&target_addr)?
                        .into_iter()
                        .next()
                        .with_context(|| {
                            format!("redirect target resolved to no addresses: {target_addr}")
                        })?;
                    crate::cluster::remember_placement(&identity.name, remote);
                    continue;
                }
                Some(Ok(WireMessage::Disconnect { reason })) => {
                    anyhow::bail!("gateway refused the session: {reason}")
                }
                other => anyhow::bail!("unexpected handshake response: {other:?}"),
            };
            validate_selected_protocol_version(handshake.protocol_version, &supported_versions)
                .context("gateway returned an incompatible MCP handshake response")?;
            break (framed, handshake);
        };
        let folder_name = root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();
        let mut session = Self {
            remote,
            framed,
            root,
            opened: HashMap::new(),
            next_id: 1,
            engine: handshake.detected_engine,
            engine_loaded: handshake.engine_age_ms.and_then(|ms| {
                std::time::Instant::now().checked_sub(std::time::Duration::from_millis(ms))
            }),
            index_gated: handshake.index_gated,
        };
        // Files the gateway lost from its copy (#262) that the sync above did not carry: the
        // next sync sends them again.
        resend_lost_files(
            &session.root,
            &gateway_node(&session.framed),
            &handshake.stale_paths,
        );
        let root_uri = prod_code_protocol::path::file_uri(&session.root);
        let init = serde_json::json!({
            "processId": null,
            "rootUri": root_uri,
            "workspaceFolders": [{ "name": folder_name, "uri": root_uri }],
            "capabilities": {
                "workspace": { "workspaceFolders": true, "configuration": true },
                "textDocument": {
                    "hover": { "contentFormat": ["markdown", "plaintext"] },
                    "definition": { "linkSupport": true },
                    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
                    "references": {}
                }
            }
        });
        session.request("initialize", init).await?;
        session.notify("initialized", serde_json::json!({})).await?;
        Ok(session)
    }
}
