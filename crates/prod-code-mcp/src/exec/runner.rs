/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use anyhow::Result;
use futures_util::SinkExt;
use prod_code_protocol::{ExecRequest, RemoteExecRequest, RemoteExecStream, WireMessage};

use super::platform::{edited_since, layout_only};
use super::progress::{ExecMonitor, prepare};
use super::types::{PolyglotRemoteOutcome, RemoteOutcome};
use crate::sync::{WorkspaceIdentity, apply_pulled_files_for, workspace_identity};

/// Runs `command` in the server copy of `root`, calling `on_output(is_stderr, bytes)` for
/// every chunk as it arrives. With `pull_changes`, files the command created, changed or
/// deleted on the server are written back into the checkout and recorded in the watermark.
#[allow(clippy::too_many_arguments)]
pub async fn run_remote(
    remote: SocketAddr,
    root: &Path,
    subdir: Option<&str>,
    command: Vec<String>,
    env: Vec<(String, String)>,
    timeout_secs: u64,
    pull_changes: bool,
    mut on_output: impl FnMut(bool, &[u8]),
) -> Result<RemoteOutcome> {
    anyhow::ensure!(!command.is_empty(), "empty command");
    let identity: WorkspaceIdentity = workspace_identity(root);
    // Taken before the pre-flight sync: a file modified after this may not be what the node
    // ran on, so the node's version must not replace it.
    let started = std::time::SystemTime::now();
    let mut framed = prepare(remote, root, &identity, &mut on_output).await?;
    let mut monitor = ExecMonitor::new(timeout_secs);
    monitor
        .send(
            &mut framed,
            WireMessage::ExecRequest(ExecRequest {
                client_workspace_root: root.to_string_lossy().to_string(),
                base_workspace_name: Some(identity.name.clone()),
                command,
                env,
                timeout_secs,
                pull_changes,
                subdir: subdir.map(str::to_string),
                client_agent: Some(prod_code_protocol::detect_client_agent()),
                client_host: Some(prod_code_protocol::client_host()),
            }),
        )
        .await?;

    let mut pulled_files = Vec::new();
    let mut relaid_files = Vec::new();
    let mut kept_files = Vec::new();
    loop {
        match monitor.next(&mut framed, &mut on_output).await? {
            Some(Ok(WireMessage::ExecChunk(chunk))) => {
                if let Some(data) = chunk.data.as_deref() {
                    on_output(chunk.stderr, data);
                }
            }
            Some(Ok(WireMessage::ExecChanges(changes))) => {
                let mut written = Vec::new();
                for delta in changes.files {
                    let local = root.join(&delta.relative_path);
                    let current = std::fs::read(&local).ok();
                    // Already the text here: a sync from this machine reached the node during
                    // the run, and the command did not make it (#254).
                    if current.as_deref() == delta.content.as_deref() {
                        continue;
                    }
                    if edited_since(&local, started) {
                        kept_files.push(delta.relative_path);
                        continue;
                    }
                    if let (Some(old), Some(new)) = (&current, delta.content.as_deref())
                        && layout_only(old, new)
                    {
                        relaid_files.push(delta.relative_path.clone());
                    }
                    written.push(delta);
                }
                pulled_files.extend(apply_pulled_files_for(root, &remote.to_string(), &written)?);
            }
            Some(Ok(WireMessage::ExecExit(exit))) => {
                let ws_name = identity.base.as_deref().unwrap_or(&identity.name);
                crate::cluster::remember_placement(ws_name, remote);
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "exec finished".to_string(),
                    })
                    .await;
                return Ok(RemoteOutcome {
                    exit,
                    pulled_files,
                    relaid_files,
                    kept_files,
                });
            }
            Some(Ok(WireMessage::Pong)) | Some(Ok(WireMessage::LspPayload(_))) => {}
            Some(Ok(other)) => anyhow::bail!("unexpected message during exec: {other:?}"),
            // A reset from a keepalive probe lands here too: the node went away without a
            // close, and whether the command finished there cannot be known (#256).
            Some(Err(e)) => {
                if !pulled_files.is_empty() {
                    anyhow::bail!(
                        "lost the connection to the gateway after receiving and applying {} changed file(s) ({e}); the command's exit code is unknown",
                        pulled_files.len()
                    );
                } else {
                    anyhow::bail!(
                        "lost the connection to the gateway during exec ({e}); the command's result is unknown"
                    );
                }
            }
            None => {
                if !pulled_files.is_empty() {
                    anyhow::bail!(
                        "gateway closed the connection after sending {} changed file(s); the command's exit code is unknown",
                        pulled_files.len()
                    );
                } else {
                    anyhow::bail!(
                        "gateway closed the connection during exec; the command's result is unknown"
                    );
                }
            }
        }
    }
}

pub async fn run_polyglot_remote(
    remote: SocketAddr,
    root: &Path,
    req: RemoteExecRequest,
    mut on_stream: impl FnMut(RemoteExecStream),
) -> Result<PolyglotRemoteOutcome> {
    let identity: WorkspaceIdentity = workspace_identity(root);
    let started = std::time::SystemTime::now();
    let mut progress = |stderr, data: &[u8]| {
        on_stream(RemoteExecStream::Chunk(prod_code_protocol::ExecChunk {
            stderr,
            data: Some(data.to_vec()),
        }))
    };
    let mut framed = prepare(remote, root, &identity, &mut progress).await?;
    let mut monitor = ExecMonitor::new(req.timeout_secs);
    monitor
        .send(&mut framed, WireMessage::RemoteExecRequest(req))
        .await?;

    let mut pulled_files = Vec::new();
    let mut relaid_files = Vec::new();
    let mut kept_files = Vec::new();
    loop {
        let mut progress = |stderr, data: &[u8]| {
            on_stream(RemoteExecStream::Chunk(prod_code_protocol::ExecChunk {
                stderr,
                data: Some(data.to_vec()),
            }))
        };
        match monitor.next(&mut framed, &mut progress).await? {
            Some(Ok(WireMessage::RemoteExecStream(stream_ev))) => {
                on_stream(stream_ev);
            }
            Some(Ok(WireMessage::ExecChanges(changes))) => {
                let mut written = Vec::new();
                for delta in changes.files {
                    let local = root.join(&delta.relative_path);
                    let current = std::fs::read(&local).ok();
                    if current.as_deref() == delta.content.as_deref() {
                        continue;
                    }
                    if edited_since(&local, started) {
                        kept_files.push(delta.relative_path);
                        continue;
                    }
                    if let (Some(old), Some(new)) = (&current, delta.content.as_deref())
                        && layout_only(old, new)
                    {
                        relaid_files.push(delta.relative_path.clone());
                    }
                    written.push(delta);
                }
                pulled_files.extend(apply_pulled_files_for(root, &remote.to_string(), &written)?);
            }
            Some(Ok(WireMessage::RemoteExecResult(result))) => {
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "remote exec finished".to_string(),
                    })
                    .await;
                return Ok(PolyglotRemoteOutcome {
                    result,
                    pulled_files,
                    relaid_files,
                    kept_files,
                });
            }
            Some(Ok(WireMessage::Pong)) | Some(Ok(WireMessage::LspPayload(_))) => {}
            Some(Ok(other)) => anyhow::bail!("unexpected message during remote exec: {other:?}"),
            Some(Err(e)) => {
                if !pulled_files.is_empty() {
                    anyhow::bail!(
                        "lost the connection to the gateway after receiving and applying {} changed file(s) ({e}); the command's exit code is unknown",
                        pulled_files.len()
                    );
                } else {
                    anyhow::bail!(
                        "lost the connection to the gateway during remote exec ({e}); the command's result is unknown"
                    );
                }
            }
            None => {
                if !pulled_files.is_empty() {
                    anyhow::bail!(
                        "gateway closed the connection after sending {} changed file(s); the command's exit code is unknown",
                        pulled_files.len()
                    );
                } else {
                    anyhow::bail!(
                        "gateway closed the connection during remote exec; the command's result is unknown"
                    );
                }
            }
        }
    }
}
