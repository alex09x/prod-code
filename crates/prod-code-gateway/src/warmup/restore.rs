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
use std::collections::HashSet;
use std::future::Future;
use std::time::{Duration, Instant};

#[cfg(test)]
mod tests;

/// What loading a workspace ahead of a request came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ahead {
    /// It was loaded.
    Loaded,
    /// The root already has a workspace loaded or loading, which is left as it is.
    Present,
    /// A worktree, which attaches to its base and is left alone.
    Worktree,
}

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
    /// The root has a workspace already, loaded or loading, of any engine.
    Present,
    /// A worktree.
    Worktree,
}

impl Skip {
    /// The low-cardinality name a metrics event carries.
    pub fn as_str(self) -> &'static str {
        match self {
            Skip::Language => "skipped_language",
            Skip::Missing => "skipped_missing",
            Skip::Busy => "skipped_busy",
            Skip::Present => "skipped_present",
            Skip::Worktree => "skipped_worktree",
        }
    }
}

/// The entries to load: the newest per root (a root is kept once, whatever its engine, so a
/// second engine of one checkout would replace the first and use a slot of its own), no more
/// than `max_workspaces`. An engine set to `never` is reported and passed over, so an older
/// engine of the same root that may be loaded still can be.
fn wanted(
    config: &WarmupConfig,
    recent: &RecentWorkspaces,
    now_ms: u64,
    report: &impl Fn(&Recent, Outcome),
) -> Vec<Recent> {
    let max_age = Duration::from_secs(config.max_age_hours.saturating_mul(3600));
    let mut roots = HashSet::new();
    let mut picked = Vec::new();
    for entry in recent.newest(usize::MAX, max_age, now_ms) {
        if picked.len() >= config.max_workspaces {
            break;
        }
        if roots.contains(&entry.root) {
            continue;
        }
        if !config.loads(&entry.engine) {
            report(&entry, Outcome::Skipped(Skip::Language));
            continue;
        }
        roots.insert(entry.root.clone());
        picked.push(entry);
    }
    picked
}

/// Loads again the workspaces of `recent` that `config` asks for, newest first, `max_concurrent`
/// at a time. `busy` is asked before each load: a node that has become busy loads no more.
/// `load` brings one workspace up, and says whether it did; `report` hears of every outcome.
pub async fn restore<L, Fut>(
    config: &WarmupConfig,
    recent: &RecentWorkspaces,
    now_ms: u64,
    busy: impl Fn() -> bool,
    load: L,
    report: impl Fn(&Recent, Outcome),
) where
    L: Fn(Recent) -> Fut,
    Fut: Future<Output = anyhow::Result<Ahead>>,
{
    if !config.after_restart {
        return;
    }
    let entries = wanted(config, recent, now_ms, &report);
    let busy = &busy;
    let load = &load;
    let report = &report;
    futures_util::stream::iter(entries)
        .for_each_concurrent(config.max_concurrent.max(1), |entry| async move {
            let outcome = if !entry.root.exists() {
                Outcome::Skipped(Skip::Missing)
            } else if busy() {
                Outcome::Skipped(Skip::Busy)
            } else {
                let started = Instant::now();
                match load(entry.clone()).await {
                    Ok(Ahead::Loaded) => Outcome::Loaded(started.elapsed()),
                    Ok(Ahead::Present) => Outcome::Skipped(Skip::Present),
                    Ok(Ahead::Worktree) => Outcome::Skipped(Skip::Worktree),
                    Err(error) => Outcome::Failed(format!("{error:#}")),
                }
            };
            report(&entry, outcome);
        })
        .await;
}
