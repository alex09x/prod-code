/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::{load_sync_cache_for, save_sync_cache_for, workspace_identity};
use crate::sync::entry::sync_file_entry;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{FileDelta, ProdCodeCodec, WireMessage};
use std::path::{Path, PathBuf};
use tokio_util::codec::Framed;

/// Writes files a remote command changed into the checkout and records them in the watermark,
/// so the next sync does not push them straight back. Returns the relative paths written or
/// deleted.
pub fn apply_pulled_files(root: &Path, files: &[FileDelta]) -> Result<Vec<String>> {
    apply_pulled_files_for(root, "", files)
}

/// [`apply_pulled_files`] recording the files as synced with gateway `node`, the node that
/// produced them.
pub fn apply_pulled_files_for(root: &Path, node: &str, files: &[FileDelta]) -> Result<Vec<String>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut state = load_sync_cache_for(&canonical_root, node);
    let mut touched = Vec::with_capacity(files.len());
    for delta in files {
        let rel = Path::new(&delta.relative_path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            continue; // never let the server write outside the checkout
        }
        let target = canonical_root.join(rel);
        match &delta.content {
            Some(content) => {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, content)?;
                #[cfg(unix)]
                if delta.is_executable {
                    use std::os::unix::fs::PermissionsExt;
                    let _ =
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755));
                }
                if let Ok(metadata) = target.metadata() {
                    state.files.insert(
                        delta.relative_path.clone(),
                        sync_file_entry(&metadata, content),
                    );
                }
            }
            None => {
                if target.exists() {
                    std::fs::remove_file(&target)?;
                }
                state.files.remove(&delta.relative_path);
            }
        }
        touched.push(delta.relative_path.clone());
    }
    if !touched.is_empty() {
        save_sync_cache_for(&canonical_root, node, &state);
        crate::call_tree::clear_call_hierarchy_cache_for(&canonical_root);
    }
    Ok(touched)
}

/// Pulls specified files from the remote gateway's workspace copy into `root`.
pub async fn pull_remote_files(
    remote: std::net::SocketAddr,
    root: &Path,
    files: &[PathBuf],
) -> Result<Vec<String>> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let identity = workspace_identity(&canonical_root);

    let mut current_remote = remote;
    let mut redirect_count = 0;
    let (mut framed, handshake_resp) = loop {
        let stream = prod_code_protocol::transport::connect(current_remote)
            .await
            .with_context(|| format!("failed to connect to remote gateway at {current_remote}"))?;
        let mut framed = Framed::new(stream, ProdCodeCodec::new());
        framed
            .send(WireMessage::HandshakeRequest(
                prod_code_protocol::HandshakeRequest {
                    protocol_version: prod_code_protocol::PROTOCOL_VERSION,
                    supported_versions: Some(prod_code_protocol::supported_protocol_versions()),
                    capabilities: Some(prod_code_protocol::ClientCapabilities {
                        redirects: true,
                        ..Default::default()
                    }),
                    client_name: "prod-code-pull".to_string(),
                    client_pid: std::process::id(),
                    auth_token: None,
                    client_workspace_root: canonical_root.to_string_lossy().to_string(),
                    preferred_engine: None,
                    base_workspace_name: Some(identity.name.clone()),
                    engine_subpath: None,
                    client_agent: Some(prod_code_protocol::detect_client_agent()),
                    client_host: Some(prod_code_protocol::client_host()),
                    purpose: None,
                    redirect_count,
                },
            ))
            .await?;

        let response = loop {
            match framed.next().await {
                Some(Ok(WireMessage::HandshakeResponse(resp))) => break Some((framed, resp)),
                Some(Ok(WireMessage::Redirect {
                    target_addr,
                    reason,
                })) => {
                    redirect_count += 1;
                    if redirect_count > 3 {
                        anyhow::bail!("too many gateway redirects during pull: {reason:?}");
                    }
                    current_remote = crate::cluster::parse_remotes(&target_addr)?
                        .into_iter()
                        .next()
                        .with_context(|| {
                            format!("redirect target resolved to no addresses: {target_addr}")
                        })?;
                    tracing::info!(%current_remote, ?reason, "following gateway redirect during pull");
                    break None;
                }
                Some(Ok(WireMessage::Auth(_))) => continue,
                Some(Ok(other)) => anyhow::bail!("unexpected message during handshake: {other:?}"),
                Some(Err(e)) => anyhow::bail!("connection error during handshake: {e}"),
                None => anyhow::bail!("gateway closed connection during handshake"),
            }
        };
        if let Some(handshake) = response {
            break handshake;
        }
    };

    let server_workspace_root = PathBuf::from(handshake_resp.server_workspace_root);
    let mut pulled_deltas = Vec::new();
    for file in files {
        let abs = if file.is_absolute() {
            file.clone()
        } else {
            canonical_root.join(file)
        };
        let mut normalized = PathBuf::new();
        for comp in abs.components() {
            match comp {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                c => normalized.push(c.as_os_str()),
            }
        }
        let rel = match normalized.strip_prefix(&canonical_root) {
            Ok(rel) => rel,
            Err(_) => continue,
        };
        let rel_str = rel.to_string_lossy().to_string();
        let remote_path = server_workspace_root.join(rel);
        framed
            .send(WireMessage::ReadFileRequest(
                prod_code_protocol::ReadFileRequest {
                    path: remote_path.to_string_lossy().to_string(),
                    max_bytes: 64 * 1024 * 1024,
                },
            ))
            .await?;

        let read_resp = loop {
            let next_msg = tokio::time::timeout(std::time::Duration::from_secs(30), framed.next())
                .await
                .map_err(|_| {
                    anyhow::anyhow!("timed out reading remote file {rel_str} from gateway")
                })?;
            match next_msg {
                Some(Ok(WireMessage::ReadFileResponse(resp))) => break resp,
                Some(Ok(WireMessage::Pong)) => continue,
                Some(Ok(other)) => anyhow::bail!("unexpected message reading file: {other:?}"),
                Some(Err(e)) => anyhow::bail!("connection error reading file: {e}"),
                None => anyhow::bail!("gateway closed connection while reading file"),
            }
        };

        if let Some(err) = read_resp.error {
            anyhow::bail!("failed to pull remote file {rel_str}: {err}");
        }

        if read_resp.truncated {
            anyhow::bail!("remote file {rel_str} exceeded maximum pull size limit (truncated)");
        }

        let is_executable = match read_resp.is_executable {
            Some(explicit) => explicit,
            None => read_resp.content.as_deref().is_some_and(|b| {
                b.starts_with(b"\x7fELF")
                    || b.starts_with(b"#!")
                    || b.starts_with(&[0xcf, 0xfa, 0xed, 0xfe])
                    || b.starts_with(&[0xfe, 0xed, 0xfa, 0xcf])
            }),
        };

        pulled_deltas.push(FileDelta {
            relative_path: rel_str,
            content: read_resp.content,
            is_executable,
        });
    }

    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "pull finished".to_string(),
        })
        .await;

    if pulled_deltas.is_empty() {
        return Ok(Vec::new());
    }

    apply_pulled_files_for(&canonical_root, &current_remote.to_string(), &pulled_deltas)
}
