/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use prod_code_mcp::diagnostics::{
    DiagnosticsReport, ValidationProgress, validate_texts_interactive_with_progress,
};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::time::{Instant, MissedTickBehavior};

/// Like exec's quiet-progress monitor: keep quick requests silent and make a
/// slow request's current phase visible without contaminating JSON on stdout.
pub(super) async fn validate_with_progress(
    remote: SocketAddr,
    root: &Path,
    edits: &[(PathBuf, String)],
) -> Result<Vec<DiagnosticsReport>> {
    let progress = ValidationProgress::new(edits.len());
    let validation =
        validate_texts_interactive_with_progress(remote, root, edits, &[], progress.clone());
    tokio::pin!(validation);
    let started = Instant::now();
    let mut reports =
        tokio::time::interval_at(started + Duration::from_secs(10), Duration::from_secs(15));
    reports.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            result = &mut validation => return result,
            _ = reports.tick() => {
                let (phase, done, total) = progress.snapshot();
                eprintln!("[prod-code validate] {phase}; {done}/{total} files done ({}s elapsed)", started.elapsed().as_secs());
            }
        }
    }
}
