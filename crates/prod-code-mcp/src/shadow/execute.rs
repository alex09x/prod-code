/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::deadline::Deadline;
use super::diff::unified_diff;
use super::rank::{rank, test_counts};
use super::types::{HypothesisOutcome, HypothesisSpec, ShadowOutcome};
use crate::sync::{WorkspaceIdentity, push_workspace_sync, workspace_identity};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{FileDelta, ShadowHypothesis, ShadowRunRequest, WireMessage};
use std::net::SocketAddr;
use std::path::Path;

/// Performs one shadow request and builds its local report.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_shadow_once(
    remote: SocketAddr,
    root: &Path,
    subdir: Option<&str>,
    specs: &[HypothesisSpec],
    command: Vec<String>,
    env: Vec<(String, String)>,
    timeout_secs: u64,
    parallel: usize,
    tail_bytes: usize,
    in_memory: bool,
) -> Result<ShadowOutcome> {
    anyhow::ensure!(!command.is_empty(), "empty command");
    anyhow::ensure!(!specs.is_empty(), "no hypotheses");
    // One deadline for the whole run, connection and sync included: only the reply wait was
    // bounded, so a sync of a large checkout ran minutes past `timeout_secs` (#2064).
    let deadline = Deadline::new(timeout_secs, specs.len(), parallel);
    let identity: WorkspaceIdentity = workspace_identity(root);
    // A shadow run syncs and runs commands: the pin is confirmed before either (#1793).
    let mut framed = deadline
        .bound(
            "connecting to the gateway",
            crate::cluster::connect_gated(remote, root, "this shadow run"),
        )
        .await?
        .framed;
    deadline
        .bound("syncing the workspace to the gateway", async {
            push_workspace_sync(&mut framed, root, &identity, None)
                .await
                .context("pre-flight workspace sync failed")
        })
        .await?;
    let hypotheses = specs
        .iter()
        .map(|spec| ShadowHypothesis {
            name: spec.name.clone(),
            files: spec
                .edits
                .iter()
                .map(|edit| FileDelta {
                    relative_path: edit.relative_path.clone(),
                    content: edit.text.as_ref().map(|t| t.as_bytes().to_vec()),
                    is_executable: edit.text.is_some()
                        && existing_root_file_is_executable(root, &edit.relative_path),
                })
                .collect(),
        })
        .collect();
    let request = framed.send(WireMessage::ShadowRunRequest(ShadowRunRequest {
        client_workspace_root: root.to_string_lossy().to_string(),
        base_workspace_name: Some(identity.name.clone()),
        hypotheses,
        command: command.clone(),
        env,
        timeout_secs,
        subdir: subdir.map(str::to_string),
        parallel,
        tail_bytes,
        in_memory,
        client_agent: Some(prod_code_protocol::detect_client_agent()),
        client_host: Some(prod_code_protocol::client_host()),
    }));
    deadline
        .bound("sending the hypotheses", async {
            request.await.map_err(anyhow::Error::from)
        })
        .await?;
    let response = loop {
        match tokio::time::timeout_at(deadline.at, framed.next()).await {
            Ok(Some(Ok(WireMessage::ShadowRunResponse(response)))) => {
                let _ = framed
                    .send(WireMessage::Disconnect {
                        reason: "shadow run finished".to_string(),
                    })
                    .await;
                break response;
            }
            Ok(Some(Ok(WireMessage::Pong))) | Ok(Some(Ok(WireMessage::LspPayload(_)))) => {}
            Ok(Some(Ok(other))) => anyhow::bail!("unexpected message during shadow run: {other:?}"),
            Ok(Some(Err(e))) => anyhow::bail!("frame decode error during shadow run: {e}"),
            Ok(None) => anyhow::bail!("gateway closed the connection during the shadow run"),
            Err(_) => return Err(deadline.expired("waiting for the gateway's result")),
        }
    };
    if let Some(error) = response.error {
        anyhow::bail!("shadow run refused: {error}");
    }
    let results: Vec<HypothesisOutcome> = response
        .results
        .into_iter()
        .map(|r| {
            let output =
                String::from_utf8_lossy(r.output_tail.as_deref().unwrap_or_default()).into_owned();
            let (diff, changed_lines) = specs
                .iter()
                .find(|s| s.name == r.name)
                .map(|s| unified_diff(root, s))
                .unwrap_or_default();
            HypothesisOutcome {
                tests: test_counts(&command, &output),
                name: r.name,
                exit_code: r.exit_code,
                duration_ms: r.duration_ms,
                timed_out: r.timed_out,
                error: r.error,
                output,
                output_len: r.output_len,
                diff,
                changed_lines,
            }
        })
        .collect();
    let ranking = rank(&results);
    let winner = ranking.first().copied().filter(|&i| results[i].passed());
    let in_memory_outcome = response.mode == "overlay-ram" || response.mode.contains("ram");
    Ok(ShadowOutcome {
        mode: response.mode,
        server_workspace_root: response.server_workspace_root,
        results,
        ranking,
        winner,
        in_memory: in_memory_outcome,
    })
}

fn existing_root_file_is_executable(root: &Path, relative_path: &str) -> bool {
    let rel = Path::new(relative_path);
    if relative_path.is_empty() || rel.is_absolute() || relative_path.chars().any(char::is_control)
    {
        return false;
    }
    let mut components = Vec::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(name) => components.push(name),
            std::path::Component::CurDir => {}
            _ => return false,
        }
    }
    if components.is_empty() {
        return false;
    }
    let mut current = root.to_path_buf();
    for (idx, name) in components.iter().enumerate() {
        current.push(name);
        let Ok(metadata) = std::fs::symlink_metadata(&current) else {
            return false;
        };
        let file_type = metadata.file_type();
        if idx + 1 < components.len() {
            if file_type.is_symlink() || !file_type.is_dir() {
                return false;
            }
        } else {
            return file_type.is_file() && crate::sync::entry::is_executable(&metadata);
        }
    }
    false
}
