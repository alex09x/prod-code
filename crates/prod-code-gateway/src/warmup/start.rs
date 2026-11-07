/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Outcome, RecentWorkspaces, WarmupConfig, restore};
use crate::metrics::Metrics;
use crate::workspace::WorkspaceManager;
use prod_code_protocol::OperationMetric;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long after the start the first load waits: the clients that were connected come back
/// first, and what they ask for is not made to queue behind what nobody has asked for.
const START_DELAY: Duration = Duration::from_secs(5);

/// The load, as a share of the cores, above which the node counts as busy.
fn busy_threshold(config: &WarmupConfig, cores: usize) -> f64 {
    f64::from(config.skip_above_load_percent) / 100.0 * cores.max(1) as f64
}

/// The metrics event of one outcome: `category=warmup`, `method=restore`.
fn event(node: &str, entry: &super::Recent, outcome: &Outcome) -> OperationMetric {
    let mut metric = OperationMetric::new(node, "warmup", "restore");
    metric.engine = entry.engine.clone();
    metric.workspace = entry
        .root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned());
    match outcome {
        Outcome::Loaded(took) => metric.duration_ms = took.as_millis() as u64,
        Outcome::Skipped(skip) => {
            metric.ok = false;
            metric.error_class = Some(skip.as_str().to_string());
        }
        Outcome::Failed(_) => {
            metric.ok = false;
            metric.error_class = Some("load_failed".to_string());
        }
    }
    metric
}

/// Loads again the workspaces that were in use before the restart, as `config` says, and
/// records what became of each as a metrics event.
pub(crate) async fn run(
    manager: Arc<WorkspaceManager>,
    metrics: Arc<Metrics>,
    node: String,
    recent: Arc<RecentWorkspaces>,
    config: WarmupConfig,
) {
    if !config.after_restart {
        return;
    }
    tokio::time::sleep(START_DELAY).await;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let threshold = busy_threshold(&config, cores);
    let busy = move || crate::memory::load_average_1m().is_some_and(|load| load > threshold);
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    restore(
        &config,
        &recent,
        now_ms,
        busy,
        |entry| {
            let manager = Arc::clone(&manager);
            async move {
                manager
                    .get_or_load_untracked(&entry.root, &entry.engine)
                    .await
                    .map(|_lease| ())
            }
        },
        |entry, outcome| {
            match &outcome {
                Outcome::Loaded(took) => tracing::info!(
                    workspace = ?entry.root, engine = %entry.engine, took_ms = took.as_millis() as u64,
                    "loaded again after the restart"
                ),
                Outcome::Skipped(skip) => tracing::info!(
                    workspace = ?entry.root, engine = %entry.engine, reason = skip.as_str(),
                    "not loaded again after the restart"
                ),
                Outcome::Failed(error) => tracing::warn!(
                    workspace = ?entry.root, engine = %entry.engine, %error,
                    "could not load again after the restart"
                ),
            }
            metrics.record_operation(event(&node, entry, &outcome));
        },
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::warmup::{Recent, Skip};

    #[test]
    fn the_busy_threshold_is_a_share_of_the_cores() {
        let config = WarmupConfig::default();
        assert!((busy_threshold(&config, 32) - 25.6).abs() < 1e-9);
        assert!(
            (busy_threshold(&config, 0) - 0.8).abs() < 1e-9,
            "no cores counts as one"
        );
    }

    #[test]
    fn an_outcome_becomes_a_low_cardinality_event() {
        let entry = Recent {
            root: "/storage/workspaces/proj".into(),
            engine: "rust".into(),
            last_used_ms: 1,
        };
        let loaded = event(
            "node",
            &entry,
            &Outcome::Loaded(Duration::from_millis(1500)),
        );
        assert_eq!(
            (
                loaded.category.as_str(),
                loaded.method.as_str(),
                loaded.engine.as_str()
            ),
            ("warmup", "restore", "rust")
        );
        assert_eq!((loaded.ok, loaded.duration_ms), (true, 1500));
        assert_eq!(
            loaded.workspace.as_deref(),
            Some("proj"),
            "a name, not a path"
        );

        let busy = event("node", &entry, &Outcome::Skipped(Skip::Busy));
        assert!(!busy.ok);
        assert_eq!(busy.error_class.as_deref(), Some("skipped_busy"));

        let failed = event("node", &entry, &Outcome::Failed("no memory".into()));
        assert_eq!(failed.error_class.as_deref(), Some("load_failed"));
        assert!(
            !format!("{failed:?}").contains("no memory"),
            "the cause stays in the log"
        );
    }
}
