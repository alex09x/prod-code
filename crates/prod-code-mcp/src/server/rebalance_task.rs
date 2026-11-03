/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The rebalance tick, off the stdio loop (#1814). Its status probes may take seconds on a
//! loaded node; run inline they stopped the loop from reading requests and writing answers, so
//! a 600 ms call answered after 3.7 s. Now one bounded check at a time runs in a task the loop
//! owns, and its verdict is applied only if the session is still on the node it asked about.

use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::time::Duration;

use super::rebalance::{apply_rebalance, rebalance_checkout, rebalance_verdict};

/// The longest one check may take: two status probes (connect and reply, about 9 s) and the
/// cluster's verdict. A check still running then is dropped and the session stays.
pub(super) const REBALANCE_BOUND: Duration = Duration::from_secs(25);

pub(super) type Verdict = Option<(SocketAddr, String)>;
type Check = Pin<Box<dyn Future<Output = Verdict> + Send>>;

/// At most one background check, aborted when the loop stops.
#[derive(Default)]
pub(super) struct Rebalancer {
    running: Option<(SocketAddr, tokio::task::JoinHandle<Verdict>)>,
}

impl Rebalancer {
    /// Starts a check of the workspace on `from`, unless one is running or the workspace is
    /// pinned: a pinned workspace never moves in the background and asks no node (#1793).
    pub(super) fn start(&mut self, from: SocketAddr, workspace_root: &Path) {
        self.start_under(&crate::cluster::node_policy(), from, workspace_root);
    }

    /// [`Rebalancer::start`] under `policy`.
    pub(super) fn start_under(
        &mut self,
        policy: &crate::cluster::NodePolicy,
        from: SocketAddr,
        workspace_root: &Path,
    ) {
        if policy.is_pinned() {
            tracing::debug!(node = %from, policy = %policy.describe(), "rebalance skipped");
            return;
        }
        let root = workspace_root.to_path_buf();
        self.start_with(
            from,
            Box::pin(async move {
                // The checkout is read with git, which blocks; keep it off the runtime's workers.
                let checkout = tokio::task::spawn_blocking(move || rebalance_checkout(&root))
                    .await
                    .ok()
                    .flatten()?;
                rebalance_verdict(from, checkout, |current, ws_name, engine, os| async move {
                    crate::cluster::evaluate_cluster_rebalance(current, &ws_name, engine, os).await
                })
                .await
            }),
        );
    }

    /// Starts `check`, asked about the workspace on `from`, bounded by [`REBALANCE_BOUND`].
    pub(super) fn start_with(&mut self, from: SocketAddr, check: Check) {
        if self.is_running() {
            return;
        }
        let bounded = async move {
            tokio::time::timeout(REBALANCE_BOUND, check)
                .await
                .unwrap_or_else(|_| {
                    tracing::warn!(node = %from, "cluster rebalance check outlived its bound");
                    None
                })
        };
        self.running = Some((from, tokio::spawn(bounded)));
    }

    pub(super) fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The finished check: the node it asked about, and its verdict. Pending while none runs;
    /// cancel-safe, so the loop can select on it.
    pub(super) async fn finished(&mut self) -> (SocketAddr, Verdict) {
        let Some((from, handle)) = self.running.as_mut() else {
            return std::future::pending().await;
        };
        let from = *from;
        let verdict = handle.await.ok().flatten();
        self.running = None;
        (from, verdict)
    }

    /// Applies a finished check to the session's `remote`; see [`apply_rebalance`].
    pub(super) fn apply(
        remote: &mut SocketAddr,
        (from, verdict): (SocketAddr, Verdict),
        workspace_root: &Path,
    ) -> bool {
        apply_rebalance(remote, from, verdict, workspace_root)
    }
}

impl Drop for Rebalancer {
    fn drop(&mut self) {
        if let Some((_, handle)) = self.running.take() {
            handle.abort();
        }
    }
}
