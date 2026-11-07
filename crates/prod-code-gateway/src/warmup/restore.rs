/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::policy::WarmupConfig;
use super::recent::{Recent, RecentWorkspaces};
use futures_util::StreamExt;
use std::future::Future;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// What became of one workspace of the record.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Loaded, in this long.
    Loaded(Duration),
    /// Left alone, and why.
    Skipped(Skip),
    /// The load failed with this.
    Failed(String),
}

/// Why a workspace was not loaded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// Its language is set to `never`.
    Language,
    /// The checkout is gone from the node's storage.
    Missing,
    /// The node was busy.
    Busy,
}

impl Skip {
    /// The low-cardinality name a metrics event carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Skip::Language => "skipped_language",
            Skip::Missing => "skipped_missing",
            Skip::Busy => "skipped_busy",
        }
    }
}

/// Loads again the workspaces of `recent` that `config` asks for, newest first, `max_concurrent`
/// at a time. `busy` is asked before each load: a node that has become busy loads no more.
/// `load` brings one workspace up; `report` hears of every outcome.
pub async fn restore<L, Fut>(
    config: &WarmupConfig,
    recent: &RecentWorkspaces,
    now_ms: u64,
    busy: impl Fn() -> bool,
    load: L,
    report: impl Fn(&Recent, Outcome),
) where
    L: Fn(Recent) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    if !config.after_restart {
        return;
    }
    let max_age = Duration::from_secs(config.max_age_hours.saturating_mul(3600));
    let wanted = recent.newest(config.max_workspaces, max_age, now_ms);
    let busy = &busy;
    let load = &load;
    let report = &report;
    futures_util::stream::iter(wanted)
        .for_each_concurrent(config.max_concurrent.max(1), |entry| async move {
            let outcome = if !config.loads(&entry.engine) {
                Outcome::Skipped(Skip::Language)
            } else if !entry.root.exists() {
                Outcome::Skipped(Skip::Missing)
            } else if busy() {
                Outcome::Skipped(Skip::Busy)
            } else {
                let started = Instant::now();
                match load(entry.clone()).await {
                    Ok(()) => Outcome::Loaded(started.elapsed()),
                    Err(error) => Outcome::Failed(format!("{error:#}")),
                }
            };
            report(&entry, outcome);
        })
        .await;
}
