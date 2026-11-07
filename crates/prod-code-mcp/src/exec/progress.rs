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
use prod_code_protocol::{AnyStream, ProdCodeCodec, WireMessage};
use std::{net::SocketAddr, path::Path, time::Duration};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::codec::Framed;

use super::types::Executor;
use crate::sync::{UploadExclusion, WorkspaceIdentity, push::push_workspace_sync_with_progress};

const REPORT_EVERY: Duration = Duration::from_secs(10);
const SYNC_LIMIT: Duration = Duration::from_secs(15 * 60);
const IDLE_LIMIT: Duration = Duration::from_secs(30);
const RESULT_GRACE: Duration = Duration::from_secs(30);

fn report(output: &mut impl FnMut(bool, &[u8]), phase: &str, started: Instant) {
    output(
        true,
        format!(
            "[prod-code exec] {phase} ({}s elapsed)\n",
            started.elapsed().as_secs()
        )
        .as_bytes(),
    );
}

/// A connection synced and ready for a command.
pub(super) struct Prepared {
    pub framed: Framed<AnyStream, ProdCodeCodec>,
    /// The root of the workspace's copy on the node, which the pre-flight sync learns before
    /// the command starts (#1479).
    pub server_root: String,
    /// The node the command will run on, as confirmed on this connection (#1793).
    pub executor: Executor,
    /// Unignored local checkout files omitted from the mirror during pre-flight sync (#1836).
    pub sync_omitted: Vec<UploadExclusion>,
}

pub(super) async fn prepare_rooted(
    remote: SocketAddr,
    root: &Path,
    identity: &WorkspaceIdentity,
    output: &mut impl FnMut(bool, &[u8]),
) -> Result<Prepared> {
    prepare_reporting_after(remote, root, identity, output, REPORT_EVERY).await
}

/// [`prepare_rooted`], naming each pre-flight step it reaches once `quiet_for` has passed: a quick
/// exec prints nothing extra, a slow one says where it waits.
pub(super) async fn prepare_reporting_after(
    remote: SocketAddr,
    root: &Path,
    identity: &WorkspaceIdentity,
    output: &mut impl FnMut(bool, &[u8]),
    quiet_for: Duration,
) -> Result<Prepared> {
    let started = Instant::now();
    // No command, and no sync that prepares one, reaches a node outside the checkout's pin
    // (#1793).
    let policy = crate::cluster::policy_for_root(root)?;
    policy.check(remote, "this command")?;
    let stream = tokio::time::timeout(IDLE_LIMIT, prod_code_protocol::transport::connect(remote))
        .await
        .context("timed out connecting to gateway; check `prod-code status`")?
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    // A strict pin is confirmed by the gateway first: one too old to hold it is refused here.
    let confirmed = crate::cluster::confirm_gate(&mut framed, remote, root, &policy).await?;
    let executor = Executor::new(remote, confirmed);
    let server_root;
    let sync_omitted;
    {
        let (phases, mut changes) = rapidfire::mpsc::unbounded();
        let mut observer = move |phase| {
            // The observer is not async, and an unbounded queue is never full.
            let _ = phases.try_send(phase);
        };
        let sync =
            push_workspace_sync_with_progress(&mut framed, root, identity, None, &mut observer);
        tokio::pin!(sync);
        let mut phase = "pre-flight workspace sync";
        let mut reports = tokio::time::interval_at(Instant::now() + REPORT_EVERY, REPORT_EVERY);
        reports.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let limit = tokio::time::sleep(SYNC_LIMIT);
        tokio::pin!(limit);
        loop {
            tokio::select! {
                result = &mut sync => {
                    let outcome = result.context("pre-flight workspace sync failed")?;
                    server_root = outcome.server_workspace_root;
                    sync_omitted = outcome.omitted;
                    break;
                }
                // A quick pre-flight stays silent; one that drags on says each step it reaches.
                Ok(next) = changes.recv() => {
                    phase = next;
                    if started.elapsed() >= quiet_for {
                        report(output, phase, started);
                    }
                }
                _ = reports.tick() => report(output, phase, started),
                _ = &mut limit => anyhow::bail!(
                    "pre-flight exec timed out while {phase}; the command was not started. Retry; check `prod-code status` if this repeats"
                ),
            }
        }
    }
    Ok(Prepared {
        framed,
        server_root,
        executor,
        sync_omitted,
    })
}

/// Keeps silent commands visible, probes the connection, and bounds missing terminal results.
pub(super) struct ExecMonitor {
    started: Instant,
    deadline: Instant,
    idle_deadline: Instant,
    reports: tokio::time::Interval,
}

impl ExecMonitor {
    pub fn new(timeout_secs: u64) -> Self {
        let started = Instant::now();
        let command_limit = Duration::from_secs(if timeout_secs == 0 {
            3600
        } else {
            timeout_secs
        });
        let deadline = started
            .checked_add(command_limit.saturating_add(RESULT_GRACE))
            .unwrap_or(started + Duration::from_secs(3600));
        let mut reports = tokio::time::interval_at(started + REPORT_EVERY, REPORT_EVERY);
        reports.set_missed_tick_behavior(MissedTickBehavior::Delay);
        Self {
            started,
            deadline,
            idle_deadline: started + IDLE_LIMIT,
            reports,
        }
    }

    pub async fn send(
        &self,
        framed: &mut Framed<AnyStream, ProdCodeCodec>,
        message: WireMessage,
    ) -> Result<()> {
        tokio::time::timeout_at(self.deadline.min(self.idle_deadline), framed.send(message))
            .await.context("gateway stopped accepting exec messages; the command's result is unknown; check `prod-code status`")??;
        Ok(())
    }

    pub async fn next(
        &mut self,
        framed: &mut Framed<AnyStream, ProdCodeCodec>,
        output: &mut impl FnMut(bool, &[u8]),
    ) -> Result<Option<Result<WireMessage, std::io::Error>>> {
        loop {
            tokio::select! {
                _ = tokio::time::sleep_until(self.deadline.min(self.idle_deadline)) => {
                    anyhow::bail!("timed out waiting for exec output or exit from gateway; the command's result is unknown. Check `prod-code status` before retrying")
                }
                frame = framed.next() => {
                    self.idle_deadline = Instant::now() + IDLE_LIMIT;
                    return Ok(frame);
                }
                _ = self.reports.tick() => {
                    report(output, "waiting for command output or exit; checking gateway connection", self.started);
                    self.send(framed, WireMessage::Ping).await?;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod tests;
