/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::report::DivergentBenchReport;
use super::session::{close_session, hover_in_session, initial_sync, open_session, query_once};
use super::setup::setup;
use super::types::{
    DivergentBenchConfig, DivergentWorktree, Expectations, MIN_WORKERS, QueryOutcome,
    WorkspaceMode, WorktreeKind,
};
use super::verify::verify;
use crate::LatencyStats;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Runs the full divergent-worktree benchmark: sets up worktrees, syncs them to `config.remote`,
/// fires concurrent workers' queries, and verifies correctness. Requires at least
/// [`MIN_WORKERS`] concurrent workers to faithfully simulate a real agent fleet.
pub async fn run(config: DivergentBenchConfig) -> Result<DivergentBenchReport> {
    if config.workers < MIN_WORKERS {
        bail!(
            "divergent benchmark requires at least {MIN_WORKERS} concurrent workers, got {}",
            config.workers
        );
    }
    if config.queries_per_worker == 0 {
        bail!("queries_per_worker must be at least 1");
    }
    if config.worktrees == 0 || !config.worktrees.is_multiple_of(4) {
        bail!(
            "--worktrees must be a multiple of 4, one copy of each mutation kind per four, got {}",
            config.worktrees
        );
    }

    let mut owned_tempdir: Option<tempfile::TempDir> = None;
    let workdir = match &config.workdir {
        Some(p) => {
            std::fs::create_dir_all(p)
                .with_context(|| format!("failed to create workdir {p:?}"))?;
            p.clone()
        }
        None => {
            let dir = tempfile::tempdir().context("failed to create scratch workdir")?;
            let path = dir.path().to_path_buf();
            owned_tempdir = Some(dir);
            path
        }
    };

    let setup = setup(
        config.base_repo.as_deref(),
        &workdir,
        config.mode,
        config.worktrees / 4,
    )?;
    let language = setup.target.language;
    let expect = Expectations::for_target(&setup.target);
    let worktrees = setup.worktrees;

    // One full sync per server workspace before the fleet starts querying.
    let mut initial_syncs = Vec::new();
    match config.mode {
        WorkspaceMode::Shared => {
            let master = worktrees
                .iter()
                .find(|w| w.kind == WorktreeKind::Master)
                .ok_or_else(|| anyhow!("master worktree missing"))?;
            initial_syncs
                .push(initial_sync(config.remote, &master.root, &master.workspace_name).await?);
        }
        WorkspaceMode::Isolated => {
            // Production worktrees inherit the origin's files and warm dependencies.
            // Load it before seeding the copies, outside the measured query wave (#408).
            initial_syncs
                .push(initial_sync(config.remote, &setup.origin, &setup.workspace_name).await?);
            let origin = DivergentWorktree {
                kind: WorktreeKind::Master,
                root: setup.origin.clone(),
                query_file: setup.origin.join(&setup.target.file_rel),
                symbol: setup.target.symbol.clone(),
                workspace_name: setup.workspace_name.clone(),
            };
            query_once(
                config.remote,
                &origin,
                language,
                "divergent-bench-seed".into(),
            )
            .await
            .context("warm benchmark origin before seeding worktrees")?;
            for wt in &worktrees {
                initial_syncs
                    .push(initial_sync(config.remote, &wt.root, &wt.workspace_name).await?);
            }
        }
    }

    let start = Instant::now();
    let mut handles = Vec::with_capacity(config.workers);
    for worker_id in 0..config.workers {
        let wt = worktrees[worker_id % worktrees.len()].clone();
        let remote = config.remote;
        let queries = config.queries_per_worker;
        let persistent = config.persistent;
        let churn_percent = config.churn_percent;
        handles.push(tokio::spawn(async move {
            let mut outcomes = Vec::with_capacity(queries);
            let record = |outcomes: &mut Vec<QueryOutcome>, t0: Instant, res: Result<String>| {
                outcomes.push(match res {
                    Ok(detail) => QueryOutcome {
                        worker_id,
                        kind: wt.kind,
                        latency: t0.elapsed(),
                        ok: true,
                        detail,
                    },
                    Err(e) => QueryOutcome {
                        worker_id,
                        kind: wt.kind,
                        latency: t0.elapsed(),
                        ok: false,
                        detail: e.to_string(),
                    },
                });
            };
            let mut dropped = 0usize;
            if persistent {
                let client_name = format!("divergent-bench-worker-{worker_id}");
                let mut session = open_session(remote, &wt, client_name.clone()).await;
                for q in 0..queries {
                    let t0 = Instant::now();
                    match session.as_mut() {
                        Ok(framed) => {
                            let res = hover_in_session(framed, &wt, language, 2 + q as i64).await;
                            record(&mut outcomes, t0, res);
                            // Deterministic churn: kill this connection without a goodbye.
                            let draw = prod_code_protocol::content_hash(
                                format!("{worker_id}:{q}").as_bytes(),
                            ) % 100;
                            if churn_percent > 0 && (draw as u8) < churn_percent {
                                drop(session);
                                dropped += 1;
                                session = open_session(remote, &wt, client_name.clone()).await;
                            }
                        }
                        Err(e) => {
                            record(&mut outcomes, t0, Err(anyhow!("{e:#}")));
                            session = open_session(remote, &wt, client_name.clone()).await;
                        }
                    }
                }
                if let Ok(framed) = session {
                    close_session(framed).await;
                }
            } else {
                for q in 0..queries {
                    let client_name = format!("divergent-bench-worker-{worker_id}-{q}");
                    let t0 = Instant::now();
                    let res = query_once(remote, &wt, language, client_name).await;
                    record(&mut outcomes, t0, res);
                }
            }
            (outcomes, dropped)
        }));
    }

    let mut all_outcomes = Vec::new();
    let mut sessions_dropped = 0usize;
    for handle in handles {
        let (outcomes, dropped) = handle
            .await
            .context("divergent-bench worker task panicked")?;
        all_outcomes.extend(outcomes);
        sessions_dropped += dropped;
    }
    let elapsed = start.elapsed();

    // After churn the gateway must have retired every killed session on its own.
    let gateway_after = if config.churn_percent > 0 {
        let mut last = None;
        for _ in 0..20 {
            match prod_code_mcp::cluster::node_status(config.remote).await {
                Ok(status) => {
                    last = Some((true, status.active_sessions));
                    if status.active_sessions == 0 {
                        break;
                    }
                }
                Err(_) => last = Some((false, usize::MAX)),
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        last
    } else {
        None
    };

    // The scratch worktrees are disposable; drop their persisted sync watermarks.
    for wt in &worktrees {
        prod_code_mcp::sync::clear_sync_cache(&wt.root);
    }

    if config.keep_workdir {
        // Prevent the scratch TempDir from deleting itself on drop so it can be inspected.
        if let Some(dir) = owned_tempdir.take() {
            let _ = dir.keep();
        }
    }

    let verifications = verify(&all_outcomes, &expect);
    let total_errors = all_outcomes.iter().filter(|o| !o.ok).count();
    let all_verified = verifications.iter().all(|v| v.passed);
    let churn_ok = config.churn_percent == 0 || matches!(gateway_after, Some((true, 0)));
    let all_passed = all_verified && total_errors == 0 && churn_ok;

    let mut latencies_us: Vec<u64> = all_outcomes
        .iter()
        .filter(|o| o.ok)
        .map(|o| o.latency.as_micros() as u64)
        .collect();
    let latency = LatencyStats::from_micros(&mut latencies_us);

    let mut latency_by_kind = BTreeMap::new();
    for kind in WorktreeKind::all() {
        let mut samples: Vec<u64> = all_outcomes
            .iter()
            .filter(|o| o.ok && o.kind == kind)
            .map(|o| o.latency.as_micros() as u64)
            .collect();
        latency_by_kind.insert(kind, LatencyStats::from_micros(&mut samples));
    }

    let mut errors_by_kind = BTreeMap::new();
    for outcome in all_outcomes.iter().filter(|o| !o.ok) {
        let entry = errors_by_kind
            .entry(outcome.kind)
            .or_insert_with(|| (0usize, outcome.detail.clone()));
        entry.0 += 1;
    }

    let qps = if elapsed.as_secs_f64() > 0.0 {
        all_outcomes.len() as f64 / elapsed.as_secs_f64()
    } else {
        0.0
    };

    Ok(DivergentBenchReport {
        language,
        mode: config.mode,
        persistent: config.persistent,
        churn_percent: config.churn_percent,
        sessions_dropped,
        gateway_after,
        workspace_name: setup.workspace_name,
        target: setup.target,
        initial_syncs,
        total_queries: all_outcomes.len(),
        total_errors,
        elapsed,
        qps,
        latency,
        latency_by_kind,
        errors_by_kind,
        verifications,
        all_passed,
    })
}
