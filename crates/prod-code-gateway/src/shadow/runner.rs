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
use prod_code_protocol::AnyStream;
use prod_code_protocol::{
    ProdCodeCodec, ShadowHypothesisResult, ShadowRunRequest, ShadowRunResponse, WireMessage,
};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::codec::Framed;

use super::in_place::run_in_place;
use super::overlay::{clean, run_overlay};
use super::root::{acquire_ram_shadow_root, overlay_unavailable};
use super::sccache::ensure_sccache_server;
use super::staging::failed;
use super::types::{DEFAULT_TAIL_BYTES, DEFAULT_TIMEOUT_SECS, Job};

fn default_parallel() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 8).max(1))
        .unwrap_or(1)
}

/// Handles a `ShadowRunRequest` on a client connection: runs every hypothesis (in parallel as
/// overlays where the node supports them), answers pings meanwhile, kills everything if the
/// client leaves, and finishes with one `ShadowRunResponse`.
pub async fn run_shadow(
    state: &crate::ServerState,
    framed: &mut Framed<AnyStream, ProdCodeCodec>,
    req: ShadowRunRequest,
) -> Result<()> {
    let workspace = crate::workspace::server_workspace_path(
        &state.storage_root,
        &req.client_workspace_root,
        req.base_workspace_name.as_deref(),
    );
    let workspace_str = workspace.to_string_lossy().to_string();
    let refuse = |error: String| ShadowRunResponse {
        server_workspace_root: workspace_str.clone(),
        mode: String::new(),
        results: Vec::new(),
        error: Some(error),
    };
    let refusal = if !workspace.is_dir() {
        Some(format!(
            "workspace {workspace_str} is not synced to this gateway"
        ))
    } else if req.command.is_empty() {
        Some("empty command".to_string())
    } else if req.hypotheses.is_empty() {
        Some("no hypotheses".to_string())
    } else {
        let mut seen = std::collections::HashSet::new();
        req.hypotheses
            .iter()
            .find(|h| h.name.trim().is_empty() || !seen.insert(h.name.as_str()))
            .map(|h| format!("hypothesis name {:?} is empty or repeated", h.name))
    };
    if let Some(error) = refusal {
        framed
            .send(WireMessage::ShadowRunResponse(refuse(error)))
            .await?;
        return Ok(());
    }
    let subdir = match req.subdir.as_deref() {
        Some(sub)
            if !sub.is_empty()
                && !sub.starts_with('/')
                && !sub.split('/').any(|c| c == "..")
                && workspace.join(sub).is_dir() =>
        {
            sub.to_string()
        }
        _ => String::new(),
    };
    let timeout = Duration::from_secs(if req.timeout_secs == 0 {
        DEFAULT_TIMEOUT_SECS
    } else {
        req.timeout_secs
    });
    let tail_limit = if req.tail_bytes == 0 {
        DEFAULT_TAIL_BYTES
    } else {
        req.tail_bytes
    };
    let overlay_reason = tokio::task::spawn_blocking(overlay_unavailable)
        .await
        .unwrap_or(Some("overlay probe failed"));
    let overlay = overlay_reason.is_none();
    let ram_requested = req.in_memory
        || std::env::var("PROD_CODE_SHADOW_RAM")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    let (mut effective_shadow_root, mut is_ram, mut fallback_shadow_root) = if overlay
        && ram_requested
    {
        match acquire_ram_shadow_root(&state.storage_root) {
            Ok(Some(ram_root)) => {
                let probe_ok = (|| -> std::io::Result<()> {
                    std::fs::create_dir_all(&ram_root)?;
                    let probe = ram_root.join(format!(".probe-{}", std::process::id()));
                    std::fs::write(&probe, b"ok")?;
                    let _ = std::fs::remove_file(&probe);
                    Ok(())
                })();
                match probe_ok {
                    Ok(()) => (ram_root, true, Some(state.shadow_root.clone())),
                    Err(why) => {
                        tracing::warn!(
                            %why,
                            "🌓 [SHADOW] in-memory RAM overlay requested but RAM root is unwritable; falling back to disk overlay"
                        );
                        (state.shadow_root.clone(), false, None)
                    }
                }
            }
            Ok(None) => {
                tracing::warn!(
                    "🌓 [SHADOW] in-memory RAM overlay requested but /dev/shm is unavailable; falling back to disk overlay"
                );
                (state.shadow_root.clone(), false, None)
            }
            Err(why) => {
                tracing::warn!(
                    %why,
                    "🌓 [SHADOW] cannot claim RAM shadow namespace; falling back to disk overlay"
                );
                (state.shadow_root.clone(), false, None)
            }
        }
    } else {
        let is_ram = state.shadow_root.starts_with("/dev/shm");
        (state.shadow_root.clone(), is_ram, None)
    };

    let mode = if !overlay {
        "in-place"
    } else if is_ram {
        "overlay-ram"
    } else {
        "overlay"
    };
    let parallel = if !overlay {
        1
    } else if req.parallel == 0 {
        default_parallel()
    } else {
        req.parallel
    };
    if let Some(reason) = overlay_reason {
        tracing::info!(
            reason,
            "🌓 [SHADOW] overlay shadows unavailable; running in place"
        );
    }
    if overlay {
        if let Err(e) = std::fs::create_dir_all(&effective_shadow_root) {
            if is_ram {
                tracing::warn!(
                    why = %e,
                    "🌓 [SHADOW] cannot create RAM shadow root; falling back to disk overlay"
                );
                effective_shadow_root = state.shadow_root.clone();
                is_ram = false;
                fallback_shadow_root = None;
                std::fs::create_dir_all(&effective_shadow_root).with_context(|| {
                    format!("cannot create {}", effective_shadow_root.display())
                })?;
            } else {
                return Err(e)
                    .with_context(|| format!("cannot create {}", effective_shadow_root.display()));
            }
        }
        if let Some(fallback) = &fallback_shadow_root {
            let _ = std::fs::create_dir_all(fallback);
        }
        tokio::task::spawn_blocking(ensure_sccache_server)
            .await
            .ok();
    }
    let names: Vec<String> = req.hypotheses.iter().map(|h| h.name.clone()).collect();
    let count = names.len();
    tracing::info!(
        workspace = %workspace_str,
        command = %req.command.join(" "),
        hypotheses = count,
        mode,
        parallel,
        "🌓 [SHADOW] started"
    );
    let start = Instant::now();
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(parallel));
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let ran_in_ram = is_ram.then(|| Arc::new(std::sync::atomic::AtomicBool::new(true)));
    let mut handles = Vec::with_capacity(count);
    for hypothesis in req.hypotheses {
        let job = Job {
            name: hypothesis.name,
            files: hypothesis.files,
            argv: req.command.clone(),
            env: req.env.clone(),
            timeout,
            tail_limit,
            workspace: workspace.clone(),
            subdir: subdir.clone(),
            shadow_root: effective_shadow_root.clone(),
            fallback_shadow_root: fallback_shadow_root.clone(),
            ran_in_ram: ran_in_ram.clone(),
            nonce,
        };
        let semaphore = Arc::clone(&semaphore);
        let cancel = cancel_rx.clone();
        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire_owned().await;
            if *cancel.borrow() {
                return failed(&job.name, "cancelled: the client left".to_string());
            }
            if overlay {
                run_overlay(job, cancel).await
            } else {
                run_in_place(job, cancel).await
            }
        }));
    }
    let all = futures_util::future::join_all(handles);
    tokio::pin!(all);
    let joined = loop {
        tokio::select! {
            joined = &mut all => break joined,
            incoming = framed.next() => match incoming {
                Some(Ok(WireMessage::Ping)) => framed.send(WireMessage::Pong).await?,
                Some(Ok(WireMessage::Disconnect { .. })) | None => {
                    let _ = cancel_tx.send(true);
                    let _ = (&mut all).await;
                    tracing::info!(workspace = %workspace_str, "🌓 [SHADOW] client left; hypotheses killed");
                    return Ok(());
                }
                Some(Err(e)) => {
                    let _ = cancel_tx.send(true);
                    let _ = (&mut all).await;
                    return Err(e.into());
                }
                _ => {}
            }
        }
    };
    if overlay {
        tokio::task::spawn_blocking(ensure_sccache_server)
            .await
            .ok();
    }
    let results: Vec<ShadowHypothesisResult> = joined
        .into_iter()
        .zip(names.iter())
        .map(|(joined, name)| {
            joined.unwrap_or_else(|e| failed(name, format!("hypothesis task failed: {e}")))
        })
        .collect();
    for result in &results {
        let mut ev = crate::metrics::Event::blank("shadow");
        ev.agent = req
            .client_agent
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.host = req
            .client_host
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        ev.workspace = workspace
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        ev.command = format!("{} [{}]", req.command.join(" "), result.name);
        ev.duration_ms = result.duration_ms;
        ev.exit_code = result.exit_code;
        ev.ok = clean(result);
        ev.bytes = result.output_len;
        state.metrics.record(ev);
    }
    let actually_ran_in_ram = is_ram
        && ran_in_ram
            .as_ref()
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed));
    let final_mode = if !overlay {
        "in-place"
    } else if actually_ran_in_ram {
        "overlay-ram"
    } else {
        "overlay"
    };
    tracing::info!(
        workspace = %workspace_str,
        hypotheses = count,
        passed = results.iter().filter(|r| clean(r)).count(),
        duration_ms = start.elapsed().as_millis() as u64,
        mode = final_mode,
        "🌓 [SHADOW] finished"
    );
    framed
        .send(WireMessage::ShadowRunResponse(ShadowRunResponse {
            server_workspace_root: workspace_str,
            mode: final_mode.to_string(),
            results,
            error: None,
        }))
        .await?;
    Ok(())
}
