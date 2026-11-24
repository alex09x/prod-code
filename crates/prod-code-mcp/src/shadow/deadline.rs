/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The deadline of one shadow run as the client sees it: every stage, from connecting to the
//! gateway's reply, ends by it with an error that names the stage (#2064).

use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

/// The gateway's own command timeout when the request gives none.
const GATEWAY_DEFAULT_TIMEOUT_SECS: u64 = 3600;
/// Room for connecting, syncing, queueing and the reply, on top of the commands' time.
pub(crate) const GRACE_SECS: u64 = 120;

pub(crate) struct Deadline {
    pub(crate) at: Instant,
    total_secs: u64,
}

impl Deadline {
    /// `timeout_secs` per hypothesis, run `parallel` at a time (0: all at once, as the
    /// gateway may; the conservative bound then assumes one at a time).
    pub(crate) fn new(timeout_secs: u64, hypotheses: usize, parallel: usize) -> Self {
        let per_run = if timeout_secs == 0 {
            GATEWAY_DEFAULT_TIMEOUT_SECS
        } else {
            timeout_secs
        };
        let hypotheses = hypotheses.max(1) as u64;
        let waves = if parallel == 0 {
            hypotheses
        } else {
            hypotheses.div_ceil(parallel as u64)
        };
        let total_secs = per_run.saturating_mul(waves).saturating_add(GRACE_SECS);
        Self {
            at: Instant::now() + Duration::from_secs(total_secs),
            total_secs,
        }
    }

    pub(crate) fn expired(&self, stage: &str) -> anyhow::Error {
        anyhow::anyhow!(
            "the shadow run did not finish within {} s (the command timeout plus {GRACE_SECS} s for connecting, syncing and queueing); it was still {stage}. Nothing was applied. Retry with fewer or smaller files, or check the node with `prod-code status`",
            self.total_secs
        )
    }

    pub(crate) async fn bound<T>(
        &self,
        stage: &str,
        work: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        match tokio::time::timeout_at(self.at, work).await {
            Ok(result) => result,
            Err(_) => Err(self.expired(stage)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_deadline_covers_every_wave_of_hypotheses_and_the_grace() {
        assert_eq!(Deadline::new(180, 1, 0).total_secs, 180 + GRACE_SECS);
        assert_eq!(Deadline::new(100, 5, 2).total_secs, 300 + GRACE_SECS);
        assert_eq!(Deadline::new(0, 1, 1).total_secs, 3600 + GRACE_SECS);
    }

    #[tokio::test]
    async fn a_stage_that_does_not_finish_names_itself() {
        let deadline = Deadline {
            at: Instant::now() + Duration::from_millis(50),
            total_secs: 121,
        };
        let error = deadline
            .bound("syncing the workspace to the gateway", async {
                std::future::pending::<anyhow::Result<()>>().await
            })
            .await
            .expect_err("a stalled stage ends at the deadline");
        let text = format!("{error:#}");
        assert!(
            text.contains("syncing the workspace to the gateway"),
            "{text}"
        );
        assert!(text.contains("within 121 s"), "{text}");
    }
}
