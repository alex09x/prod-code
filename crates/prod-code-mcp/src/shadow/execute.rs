/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::diff::unified_diff;
use super::rank::{rank, test_counts};
use super::types::{HypothesisOutcome, HypothesisSpec, ShadowOutcome};
use crate::sync::{WorkspaceIdentity, push_workspace_sync, workspace_identity};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    FileDelta, ProdCodeCodec, ShadowHypothesis, ShadowRunRequest, WireMessage,
};
use std::net::SocketAddr;
use std::path::Path;
use tokio_util::codec::Framed;

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
    let identity: WorkspaceIdentity = workspace_identity(root);
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    push_workspace_sync(&mut framed, root, &identity, None)
        .await
        .context("pre-flight workspace sync failed")?;
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
                    is_executable: false,
                })
                .collect(),
        })
        .collect();
    framed
        .send(WireMessage::ShadowRunRequest(ShadowRunRequest {
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
        }))
        .await?;
    let budget = std::time::Duration::from_secs(timeout_secs.saturating_add(30));
    let response = loop {
        match tokio::time::timeout(budget, framed.next()).await {
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
            Err(_) => anyhow::bail!("timed out waiting for shadow run response from gateway"),
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
