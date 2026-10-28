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
    FileDelta, PULL_CHUNK_BYTES, ProdCodeCodec, PullFileRequest, WireMessage,
};
use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use tokio_util::codec::Framed;

pub struct PullOutcome {
    pub node: SocketAddr,
    pub server_workspace_root: String,
    pub files: Vec<String>,
}

/// Pull from an explicit node. Never redirect or sync: ignored artifacts belong to that copy.
pub async fn pull_remote_files_from(
    remote: SocketAddr,
    root: &Path,
    files: &[PathBuf],
) -> Result<PullOutcome> {
    let root = std::fs::canonicalize(root)?;
    let identity = super::workspace_identity(&root);
    let stream = prod_code_protocol::transport::connect(remote).await
        .with_context(|| format!("cannot pull from node {remote}; use the node printed by prod-code exec (--remote NODE)"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let mut copy = String::new();
    let mut pulled = Vec::new();
    for path in files {
        let relative = if path.is_absolute() {
            path.strip_prefix(&root)?
        } else {
            path.as_path()
        };
        anyhow::ensure!(
            !relative.as_os_str().is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "pull path must be inside the checkout: {}",
            path.display()
        );
        let name = relative.to_string_lossy().into_owned();
        let mut bytes = Vec::new();
        let mut total = None;
        let executable = loop {
            let offset = bytes.len() as u64;
            framed
                .send(WireMessage::PullFileRequest(PullFileRequest {
                    client_workspace_root: root.to_string_lossy().into_owned(),
                    base_workspace_name: Some(identity.name.clone()),
                    path: name.clone(),
                    offset,
                    list_directory: false,
                }))
                .await?;
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
            let response = loop {
                let next = tokio::time::timeout_at(deadline, framed.next())
                    .await
                    .with_context(|| {
                        format!("pull {name} timed out on node {remote}, copy {copy}")
                    })?;
                match next {
                    Some(Ok(WireMessage::PullFileResponse(response))) => break response,
                    Some(Ok(
                        WireMessage::Pong | WireMessage::LspPayload(_) | WireMessage::Auth(_),
                    )) => continue,
                    None => anyhow::bail!(
                        "node {remote} closed the connection while pulling {name}; nothing was written. A gateway older than the client cannot pull artifacts: update it, or use the node printed by prod-code exec"
                    ),
                    _ => anyhow::bail!(
                        "artifact pull unavailable on node {remote}; update the gateway to support PullFileRequest and use the node printed by prod-code exec"
                    ),
                }
            };
            if copy.is_empty() {
                copy = response.server_workspace_root.clone();
            }
            anyhow::ensure!(
                response.server_workspace_root == copy && response.offset == offset,
                "artifact copy or offset changed during pull from node {remote}"
            );
            if let Some(error) = response.file.error {
                let exec_node = crate::exec::last_exec_node(&root).unwrap_or(remote);
                anyhow::bail!(
                    "failed to pull {name} on node {remote}, copy {copy}: {error}. Check the node printed by prod-code exec; retry with prod-code --remote {exec_node} pull {name} (MCP: code_pull remote={exec_node})"
                );
            }
            let chunk = response
                .file
                .content
                .context("artifact response had no content")?;
            anyhow::ensure!(
                chunk.len() as u64 <= PULL_CHUNK_BYTES,
                "oversized artifact chunk"
            );
            if let Some(expected) = total {
                anyhow::ensure!(
                    expected == response.total_bytes,
                    "artifact changed size during pull"
                );
            }
            total = Some(response.total_bytes);
            bytes.extend_from_slice(&chunk);
            anyhow::ensure!(
                bytes.len() as u64 <= response.total_bytes,
                "artifact exceeded reported size"
            );
            if !response.file.truncated {
                anyhow::ensure!(
                    bytes.len() as u64 == response.total_bytes,
                    "incomplete artifact pull"
                );
                break response.file.is_executable.unwrap_or(false);
            }
            anyhow::ensure!(!chunk.is_empty(), "artifact pull made no progress");
        };
        // Refuse local symlinks too: a remote artifact must never overwrite a host file.
        let target = root.join(relative);
        for ancestor in target.ancestors().take_while(|p| *p != root) {
            if let Ok(metadata) = std::fs::symlink_metadata(ancestor) {
                anyhow::ensure!(
                    !metadata.file_type().is_symlink(),
                    "pull destination contains a symlink: {}",
                    ancestor.display()
                );
            }
        }
        pulled.push(FileDelta {
            relative_path: name,
            content: Some(bytes),
            is_executable: executable,
        });
    }
    let _ = framed
        .send(WireMessage::Disconnect {
            reason: "pull finished".into(),
        })
        .await;
    let files = super::apply_pulled_files_for(&root, &remote.to_string(), &pulled)?;
    Ok(PullOutcome {
        node: remote,
        server_workspace_root: copy,
        files,
    })
}
