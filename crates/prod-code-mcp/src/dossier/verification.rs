/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::{net::SocketAddr, path::Path};
use tokio::time::{Instant, timeout_at};

use super::locations::suggested;
use crate::{exec::run_remote, fixit::parse_fixes};

/// Rebuild exactly the original test selection for Cargo's machine-applicable fixes.
/// Ordinary check/test planning and parsing retain their existing behavior.
pub(super) async fn test_build_suggestions(
    remote: SocketAddr,
    root: &Path,
    hint: Option<&Path>,
    test_command: &[String],
    deadline: Instant,
) -> Vec<String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Vec::new();
    }
    // ExecRequest uses whole seconds; round up so a fractional remainder is neither
    // discarded nor sent as zero (which means the default timeout). The outer deadline
    // enforces the exact budget, including sync and transport waits.
    let timeout_secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0);
    let mut command = test_command.to_vec();
    command.extend(["--no-run".to_string(), "--message-format=json".to_string()]);
    let (subdir, _) = crate::sync::engine_project(root, hint.unwrap_or(root));
    let mut stdout = Vec::new();
    let outcome = timeout_at(
        deadline,
        run_remote(
            remote,
            root,
            subdir.as_deref(),
            command,
            vec![
                ("CARGO_TERM_COLOR".to_string(), "never".to_string()),
                ("NO_COLOR".to_string(), "1".to_string()),
            ],
            timeout_secs,
            false,
            |is_stderr, data| {
                if !is_stderr {
                    stdout.extend_from_slice(data);
                }
            },
        ),
    )
    .await;
    // Suggestions are best effort. Keep the original build failure if the fix pass
    // cannot finish; partial or late compiler output must not become claimed fixes.
    match outcome {
        Ok(Ok(outcome)) if !outcome.exit.timed_out && outcome.exit.error.is_none() => {
            let fixes = String::from_utf8_lossy(&stdout)
                .lines()
                .flat_map(parse_fixes)
                .collect::<Vec<_>>();
            suggested(&fixes)
        }
        _ => Vec::new(),
    }
}
