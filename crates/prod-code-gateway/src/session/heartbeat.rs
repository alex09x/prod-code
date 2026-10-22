/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Keeps a client that waits for a long answer informed that the gateway is still working.
//!
//! The first probe of a new worktree seeds its copy from the main checkout's. Four seeds at once
//! took 67-70 s on one node, past the client's 30 s wait, and every one failed with "timed out
//! waiting for sync probe response" although the gateway answered later (#1164). A `Pong` every
//! few seconds tells the client to keep waiting; a client that predates this skips them.

use futures_util::{Sink, SinkExt};
use prod_code_protocol::WireMessage;
use std::future::Future;
use std::time::Duration;

/// How often a long-running answer is preceded by a keepalive.
pub(crate) const HEARTBEAT_EVERY: Duration = Duration::from_secs(5);

/// Runs `work`, sending `WireMessage::Pong` on `sink` every `every` until it finishes.
pub(crate) async fn with_heartbeat<T, S>(
    sink: &mut S,
    every: Duration,
    work: impl Future<Output = T>,
) -> anyhow::Result<T>
where
    S: Sink<WireMessage> + Unpin,
    S::Error: Into<anyhow::Error>,
{
    tokio::pin!(work);
    let mut beat = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    beat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            done = &mut work => return Ok(done),
            _ = beat.tick() => sink.send(WireMessage::Pong).await.map_err(Into::into)?,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_long_answer_is_preceded_by_keepalives_and_a_quick_one_by_none() {
        let mut sent: Vec<WireMessage> = Vec::new();
        let answer = with_heartbeat(&mut sent, Duration::from_millis(20), async {
            tokio::time::sleep(Duration::from_millis(110)).await;
            42
        })
        .await
        .unwrap();
        assert_eq!(answer, 42);
        assert!(
            sent.len() >= 3 && sent.iter().all(|m| matches!(m, WireMessage::Pong)),
            "{} keepalives in 110 ms at 20 ms: {sent:?}",
            sent.len()
        );

        let mut sent: Vec<WireMessage> = Vec::new();
        with_heartbeat(&mut sent, Duration::from_secs(5), async {})
            .await
            .unwrap();
        assert!(sent.is_empty(), "a quick answer needs no keepalive");
    }
}
