/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Periodically unloads idle engines and prunes stale worktree workspace directories.
/// How long an engine may sit idle on a host short of memory before it is unloaded, however
/// long `--idle-evict-secs` lets it stay otherwise (#396).
pub(crate) const PRESSURE_EVICT_IDLE: Duration = Duration::from_secs(300);

/// After how long idle engines are unloaded: `--idle-evict-secs` (0 keeps them), cut to
/// [`PRESSURE_EVICT_IDLE`] while the host is short of memory, even when eviction is off.
pub(crate) fn evict_after(idle_evict_secs: u64, memory_short: bool) -> Option<Duration> {
    let configured = (idle_evict_secs > 0).then(|| Duration::from_secs(idle_evict_secs));
    if memory_short {
        Some(configured.map_or(PRESSURE_EVICT_IDLE, |c| c.min(PRESSURE_EVICT_IDLE)))
    } else {
        configured
    }
}

pub(crate) async fn janitor(
    state: Arc<ServerState>,
    idle_evict_secs: u64,
    prune_worktree_secs: u64,
    prune_workspace_secs: u64,
    prune_below_free_percent: u64,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
    ticker.tick().await;
    let mut was_short: Option<String> = None;
    loop {
        ticker.tick().await;
        // An engine installed while the daemon runs is picked up here, on a thread that is
        // allowed to block, instead of by the next request that needs the list.
        let _ = tokio::task::spawn_blocking(refresh_available_engines).await;
        // The memory and disk watchdog (#396): placement already keeps new workspaces off a
        // node that is short; the log says when it starts and stops being short.
        let host = memory::host_resources(&state.storage_root);
        let short = host.pressure();
        match (&was_short, &short) {
            (None, Some(why)) => tracing::warn!(
                %why,
                "⚠️ [PRESSURE] this node is short: new workspaces go to other nodes"
            ),
            (Some(_), None) => {
                tracing::info!(now = %host.describe(), "✅ [PRESSURE] this node has room again")
            }
            _ => {}
        }
        was_short = short;
        if was_short.is_some() {
            let view = state.cluster_view().await;
            let own_addr = state.advertise.read().await.clone();
            let ws_summary = state
                .workspace_manager
                .loaded_workspaces_for_rebalance()
                .await;
            for (ws, active) in ws_summary {
                if active > 0 {
                    let ws_name = ws
                        .root
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let os = if ws.engine == "swift" {
                        Some("macos".to_string())
                    } else {
                        None
                    };
                    let place_req = PlaceRequest {
                        workspace_name: ws_name,
                        engine: Some(ws.engine.clone()),
                        os,
                        rebalance_active: true,
                    };
                    let place_resp = place_in(&place_req, view.clone());
                    if let Some(target) = place_resp.node {
                        if target != own_addr && target != view.this_node {
                            let notified = ws.trigger_rebalance(
                                target.clone(),
                                Some("evacuating node under memory pressure".to_string()),
                            );
                            if notified > 0 {
                                tracing::info!(
                                    workspace = %ws.root.display(),
                                    target = %target,
                                    notified,
                                    "rebalanced active workspace to compatible peer under host pressure"
                                );
                                break;
                            }
                        }
                    }
                }
            }
        }
        let memory_short = host
            .memory_used_share()
            .is_some_and(|used| used > prod_code_protocol::MEMORY_PRESSURE_USED);
        if let Some(after) = evict_after(idle_evict_secs, memory_short) {
            let evicted = state.workspace_manager.evict_idle(after).await;
            for root in evicted {
                tracing::info!(workspace = %root.display(), idle_secs = after.as_secs(), memory_short, "💤 [EVICT] unloaded idle workspace engine");
            }
        }
        if prune_worktree_secs > 0 {
            let pruned = workspace::prune_stale_worktree_dirs(
                &state.storage_root,
                std::time::Duration::from_secs(prune_worktree_secs),
                &state.workspace_manager,
            )
            .await;
            for path in pruned {
                state.search_indexes.forget(&path);
            }
        }
        if prune_workspace_secs > 0 {
            let pruned = workspace::prune_stale_main_workspace_dirs(
                &state.storage_root,
                std::time::Duration::from_secs(prune_workspace_secs),
                std::time::Duration::from_secs(prune_worktree_secs),
                &state.workspace_manager,
            )
            .await;
            for path in pruned {
                state.search_indexes.forget(&path);
            }
        }
        if prune_below_free_percent > 0 {
            let pruned = workspace::prune_worktree_dirs_for_space(
                &state.storage_root,
                prune_below_free_percent as f64 / 100.0,
                &state.workspace_manager,
                workspace::free_share,
            )
            .await;
            for path in pruned {
                state.search_indexes.forget(&path);
            }
        }
        if state.build_cache_ram {
            let build_cache_base = state.build_cache_dir.clone().unwrap_or_else(|| {
                if Path::new("/dev/shm").is_dir() {
                    PathBuf::from("/dev/shm/prod-code-build")
                } else {
                    PathBuf::from("/tmp/prod-code-build")
                }
            });
            let _ = tokio::task::spawn_blocking(move || sweep_ram_build_caches(&build_cache_base))
                .await;
        }
        let _ = tokio::task::spawn_blocking(|| {
            let _ = python_cache::prune_stale_stub_cache(
                std::time::Duration::from_secs(7 * 86400),
                5 * 1024 * 1024 * 1024,
            );
            let _ = swift_cache::prune_stale_module_cache(
                std::time::Duration::from_secs(7 * 86400),
                10 * 1024 * 1024 * 1024,
            );
            let _ = ts_cache::prune_stale_types_cache(
                std::time::Duration::from_secs(7 * 86400),
                5 * 1024 * 1024 * 1024,
            );
        })
        .await;
    }
}

