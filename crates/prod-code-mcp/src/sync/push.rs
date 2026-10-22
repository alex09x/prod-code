/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::cache::{
    PROBED_NODES, clear_sync_cache_for, resend_lost_files, workspace_sync_lock,
};
use crate::sync::entry::{sync_batches, sync_file_entry};
use crate::sync::plan::{commit_workspace_sync, prepare_workspace_sync_for};
use crate::sync::read::read_file_or_contained_symlink;
use crate::sync::types::WorkspaceIdentity;
use crate::sync::types::{RoundResult, SYNC_BATCH_BYTES, SyncOutcome};
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{
    AnyStream, FileDelta, ProdCodeCodec, SyncProbeRequest, SyncRequest, WireMessage,
};
use std::collections::HashSet;
use std::path::Path;
use tokio::time::Duration;
use tokio_util::codec::Framed;

async fn wait_for_message<T>(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    what: &str,
    pick: impl Fn(WireMessage) -> Option<T>,
) -> Result<T> {
    let timeout_secs = if what.contains("probe") { 30 } else { 180 };
    wait_for_message_within(framed, what, Duration::from_secs(timeout_secs), pick).await
}

async fn wait_for_message_within<T>(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    what: &str,
    limit: Duration,
    pick: impl Fn(WireMessage) -> Option<T>,
) -> Result<T> {
    wait_for_message_capped(framed, what, limit, MAX_SYNC_WAIT, pick).await
}

/// The longest a sync or probe is waited for while the gateway keeps sending keepalives.
const MAX_SYNC_WAIT: Duration = Duration::from_secs(15 * 60);

/// Waits until `limit` passes with no frame at all, or `cap` in total. A gateway that is still
/// working, seeding a new worktree's copy say, sends keepalives meanwhile (#1164).
async fn wait_for_message_capped<T>(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    what: &str,
    limit: Duration,
    cap: Duration,
    pick: impl Fn(WireMessage) -> Option<T>,
) -> Result<T> {
    let started = tokio::time::Instant::now();
    let mut deadline = started + limit;
    // Says where it waited and what is known, so a stuck node is not mistaken for a lost
    // upload (#1143).
    let node = gateway_node(framed);
    let timed_out = || {
        anyhow::anyhow!(
            "timed out after {}s waiting for {what} from gateway {node}; files sent \
             before the timeout may already be on that node. Retry the command; if it times \
             out again, check the node with `prod-code status`",
            started.elapsed().as_secs()
        )
    };
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(timed_out());
        }
        match tokio::time::timeout(remaining, framed.next()).await {
            Ok(Some(Ok(WireMessage::LspPayload(_)))) | Ok(Some(Ok(WireMessage::Pong))) => {
                deadline = (tokio::time::Instant::now() + limit).min(started + cap);
            }
            Ok(Some(Ok(WireMessage::Disconnect { reason }))) => {
                anyhow::bail!("gateway disconnected while waiting for {what}: {reason}")
            }
            Ok(Some(Ok(msg))) => match pick(msg) {
                Some(value) => return Ok(value),
                None => anyhow::bail!("unexpected message while waiting for {what}"),
            },
            Ok(Some(Err(e))) => anyhow::bail!("frame decode error while waiting for {what}: {e}"),
            Ok(None) => anyhow::bail!("gateway closed connection while waiting for {what}"),
            Err(_) => return Err(timed_out()),
        }
    }
}

/// Brings the gateway's copy of `root` up to date on an open, not yet handshaken connection.
///
/// First contact (no recorded base) sends a manifest probe so the gateway can seed the
/// workspace from the origin repository's copy and report only the files it still lacks;
/// later rounds send the watermark delta. The watermark is committed once the gateway has
/// acknowledged the uploads.
pub async fn push_workspace_sync(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    root: &Path,
    identity: &WorkspaceIdentity,
    subpath: Option<&Path>,
) -> Result<SyncOutcome> {
    const MAX_RESET_RETRIES: usize = 2;
    let sync_lock = workspace_sync_lock(root, &gateway_node(framed));
    let _sync_guard = sync_lock.lock().await;
    let mut resets = 0;
    loop {
        let mut outcome = match push_sync_round(framed, root, identity, subpath).await? {
            RoundResult::Success(outcome) => outcome,
            RoundResult::NeedsFullResync => {
                resets += 1;
                if resets > MAX_RESET_RETRIES {
                    anyhow::bail!(
                        "gateway workspace reset loop detected: server workspace remains fresh after resync for {}",
                        identity.name
                    );
                }
                continue;
            }
        };

        // The gateway removed files a command changed after its client left and it could not put
        // back (#262). They go in one more round on this connection, so that what runs next sees
        // the checkout's version of them; a second report waits for the next sync.
        if !outcome.stale_paths.is_empty() {
            resend_lost_files(root, &gateway_node(framed), &outcome.stale_paths);
            match push_sync_round(framed, root, identity, subpath).await? {
                RoundResult::Success(again) => {
                    outcome.files_updated += again.files_updated;
                    outcome.files_deleted += again.files_deleted;
                    outcome.bytes_transferred += again.bytes_transferred;
                    outcome.changed_paths.extend(again.changed_paths);
                    outcome.stale_paths = again.stale_paths;
                }
                RoundResult::NeedsFullResync => {
                    resets += 1;
                    if resets > MAX_RESET_RETRIES {
                        anyhow::bail!(
                            "gateway workspace reset loop detected: server workspace remains fresh after resync for {}",
                            identity.name
                        );
                    }
                    continue;
                }
            }
        }
        return Ok(outcome);
    }
}

/// The `host:port` of the gateway on the other end of `framed`, which keys its watermark.
pub fn gateway_node(framed: &Framed<AnyStream, ProdCodeCodec>) -> String {
    framed
        .get_ref()
        .peer_addr()
        .map(|addr| addr.to_string())
        .unwrap_or_default()
}

async fn push_sync_round(
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    root: &Path,
    identity: &WorkspaceIdentity,
    subpath: Option<&Path>,
) -> Result<RoundResult> {
    let node = gateway_node(framed);
    let mut plan = prepare_workspace_sync_for(root, &node, subpath)?;
    let root_str = root.to_string_lossy().to_string();
    let mut outcome = SyncOutcome {
        planned: plan.files.len(),
        ..SyncOutcome::default()
    };

    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let is_first_contact = subpath.is_none()
        && !PROBED_NODES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&(canonical_root.clone(), node.clone()));
    let needs_probe = plan.initial || is_first_contact;

    let mut uploading_full_manifest = false;
    if needs_probe {
        let probed_manifest = plan.manifest();
        framed
            .send(WireMessage::SyncProbeRequest(SyncProbeRequest {
                client_workspace_root: root_str.clone(),
                base_workspace_name: Some(identity.name.clone()),
                seed_from: identity.base.clone(),
                files: probed_manifest.clone(),
            }))
            .await?;
        let probe = wait_for_message(framed, "sync probe response", |m| match m {
            WireMessage::SyncProbeResponse(r) => Some(r),
            _ => None,
        })
        .await?;
        outcome.probed = true;
        outcome.seeded = probe.seeded;
        outcome.files_deleted += probe.files_deleted;
        outcome.server_workspace_root = probe.server_workspace_root;
        let keep: HashSet<String> = probe.missing.into_iter().collect();
        uploading_full_manifest = !probe.seeded
            && !probed_manifest.is_empty()
            && probed_manifest
                .iter()
                .all(|f| keep.contains(&f.relative_path));
        plan.retain_uploads(&keep);
        let existing: HashSet<String> =
            plan.files.iter().map(|f| f.relative_path.clone()).collect();
        for missing_rel in &keep {
            if !existing.contains(missing_rel) {
                let full = canonical_root.join(missing_rel);
                if let Ok(Some((content, is_exec, meta))) =
                    read_file_or_contained_symlink(&full, &canonical_root, missing_rel)
                {
                    let entry = sync_file_entry(&meta, &content);
                    plan.state.files.insert(missing_rel.clone(), entry);
                    plan.files.push(FileDelta {
                        relative_path: missing_rel.clone(),
                        content: Some(content),
                        is_executable: is_exec,
                    });
                }
            }
        }
        let uploaded_set: HashSet<&str> = plan
            .files
            .iter()
            .map(|f| f.relative_path.as_str())
            .collect();
        uploading_full_manifest &= probed_manifest
            .iter()
            .all(|f| uploaded_set.contains(f.relative_path.as_str()));
    }

    // An empty delta is still sent on later rounds: the gateway's answer tells whether its
    // copy of the workspace still exists, so a pruned or never-seen node is caught here
    // instead of failing the query or command that follows.
    if !plan.files.is_empty() || !plan.initial {
        let files = std::mem::take(&mut plan.files);
        outcome.changed_paths = files.iter().map(|f| f.relative_path.clone()).collect();
        for (i, files) in sync_batches(files, SYNC_BATCH_BYTES)
            .into_iter()
            .enumerate()
        {
            framed
                .send(WireMessage::SyncRequest(SyncRequest {
                    client_workspace_root: root_str.clone(),
                    files,
                    clean_others: false,
                    base_workspace_name: Some(identity.name.clone()),
                }))
                .await?;
            let resp = wait_for_message(framed, "sync response", |m| match m {
                WireMessage::SyncResponse(r) => Some(r),
                _ => None,
            })
            .await
            .context("workspace sync was not acknowledged")?;
            if resp.workspace_was_fresh && !(i == 0 && uploading_full_manifest) {
                // The server directory was reset behind our watermark (or between probe and
                // upload): forget it and start over with a manifest probe on the same connection.
                tracing::warn!(
                    workspace = %identity.name,
                    "gateway workspace was reset; resyncing the full tree"
                );
                clear_sync_cache_for(root, &node);
                return Ok(RoundResult::NeedsFullResync);
            }
            outcome.files_updated += resp.files_updated;
            outcome.files_deleted += resp.files_deleted;
            outcome.bytes_transferred += resp.bytes_transferred;
            outcome.server_workspace_root = resp.server_workspace_root;
            for stale in resp.stale_paths {
                if !outcome.stale_paths.contains(&stale) {
                    outcome.stale_paths.push(stale);
                }
            }
        }
    }

    commit_workspace_sync(root, &plan);
    if subpath.is_none() {
        PROBED_NODES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((canonical_root, node));
    }
    Ok(RoundResult::Success(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_silent_gateway_times_out_naming_the_node_and_the_next_step() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accept = tokio::spawn(async move { listener.accept().await.unwrap().0 });
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        // Accepted and never answered, like the stuck sync on the node in #1143.
        let _silent = accept.await.unwrap();
        let mut framed = Framed::new(AnyStream::Tcp(stream), ProdCodeCodec::new());

        let err =
            wait_for_message_within(&mut framed, "sync response", Duration::from_secs(1), |m| {
                match m {
                    WireMessage::SyncResponse(r) => Some(r),
                    _ => None,
                }
            })
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("after 1s waiting for sync response"), "{err}");
        assert!(err.contains(&addr.to_string()), "{err}");
        assert!(err.contains("may already be on that node"), "{err}");
        assert!(err.contains("prod-code status"), "{err}");
    }

    /// A gateway that sends keepalives every 300 ms; with `answer_after`, it then answers.
    async fn keepalive_gateway(answer_after: Option<Duration>) -> Framed<AnyStream, ProdCodeCodec> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut gateway = Framed::new(socket, ProdCodeCodec::new());
            let started = tokio::time::Instant::now();
            loop {
                tokio::time::sleep(Duration::from_millis(300)).await;
                if answer_after.is_some_and(|after| started.elapsed() >= after) {
                    let _ = gateway
                        .send(WireMessage::SyncProbeResponse(
                            prod_code_protocol::SyncProbeResponse {
                                server_workspace_root: "/srv/ws".to_string(),
                                seeded: true,
                                files_deleted: 0,
                                missing: Vec::new(),
                            },
                        ))
                        .await;
                    return;
                }
                if gateway.send(WireMessage::Pong).await.is_err() {
                    return;
                }
            }
        });
        let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        Framed::new(AnyStream::Tcp(stream), ProdCodeCodec::new())
    }

    fn probe_response(m: WireMessage) -> Option<prod_code_protocol::SyncProbeResponse> {
        match m {
            WireMessage::SyncProbeResponse(r) => Some(r),
            _ => None,
        }
    }

    #[tokio::test]
    async fn keepalives_extend_the_wait_past_the_idle_limit_up_to_the_cap() {
        // A seed that outlasts the 1 s idle limit is still answered (#1164).
        let mut framed = keepalive_gateway(Some(Duration::from_millis(1500))).await;
        let probe = wait_for_message_capped(
            &mut framed,
            "sync probe response",
            Duration::from_secs(1),
            Duration::from_secs(10),
            probe_response,
        )
        .await
        .expect("keepalives keep the probe alive");
        assert!(probe.seeded);

        // Keepalives without an answer stop at the cap.
        let mut framed = keepalive_gateway(None).await;
        let started = tokio::time::Instant::now();
        let err = wait_for_message_capped(
            &mut framed,
            "sync probe response",
            Duration::from_secs(1),
            Duration::from_secs(2),
            probe_response,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(started.elapsed() < Duration::from_secs(4), "{err}");
        assert!(err.contains("waiting for sync probe response"), "{err}");
    }
}
