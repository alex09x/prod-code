/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{HypothesisSpec, ShadowOutcome, run_shadow_once};
use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

/// Sends the hypotheses, waits for their results, and ranks them. A Cargo check whose shadow
/// fails only because a cached Rust `.rmeta` file is missing warms Cargo's target and retries.
#[allow(clippy::too_many_arguments)]
pub async fn run_shadow(
    remote: SocketAddr,
    root: &Path,
    subdir: Option<&str>,
    specs: &[HypothesisSpec],
    command: Vec<String>,
    env: Vec<(String, String)>,
    timeout_secs: u64,
    parallel: usize,
    tail_bytes: usize,
    in_memory: bool,
) -> Result<ShadowOutcome> {
    let first = run_shadow_once(
        remote,
        root,
        subdir,
        specs,
        command.clone(),
        env.clone(),
        timeout_secs,
        parallel,
        tail_bytes,
        in_memory,
    )
    .await?;
    if !first.results.iter().any(|result| {
        !result.passed() && needs_dependency_metadata_warmup(&command, &result.output)
    }) {
        return Ok(first);
    }

    let warmup = crate::exec::run_remote(
        remote,
        root,
        subdir,
        command.clone(),
        env.clone(),
        timeout_secs,
        false,
        |_, _| {},
    )
    .await?;
    if let Some(error) = warmup.exit.error.as_deref() {
        anyhow::bail!("cargo check could not start while warming dependency metadata: {error}");
    }
    if warmup.exit.timed_out {
        anyhow::bail!("cargo check timed out while warming missing dependency metadata");
    }

    let mut retry = run_shadow_once(
        remote,
        root,
        subdir,
        specs,
        command,
        env,
        timeout_secs,
        parallel,
        tail_bytes,
        in_memory,
    )
    .await?;
    for result in &mut retry.results {
        let first_duration = first
            .results
            .iter()
            .find(|previous| previous.name == result.name)
            .map_or(0, |previous| previous.duration_ms);
        result.duration_ms = result
            .duration_ms
            .saturating_add(first_duration)
            .saturating_add(warmup.exit.duration_ms);
    }
    Ok(retry)
}

fn needs_dependency_metadata_warmup(command: &[String], output: &str) -> bool {
    let Some(program) = command.first() else {
        return false;
    };
    let is_cargo = program.rsplit('/').next().unwrap_or(program) == "cargo";
    let command_is_check = command.get(1).is_some_and(|arg| arg == "check")
        || (command.get(1).is_some_and(|arg| arg.starts_with('+'))
            && command.get(2).is_some_and(|arg| arg == "check"));
    is_cargo
        && command_is_check
        && output.lines().any(|line| {
            let line = line.to_ascii_lowercase();
            line.contains(".rmeta")
                && (line.contains("does not exist")
                    || line.contains("no such file")
                    || line.contains("not found"))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_missing_rmeta_files_from_cargo_check_trigger_a_warmup() {
        let cargo_check = vec!["/usr/bin/cargo".to_string(), "check".to_string()];
        let cargo_toolchain_check = vec![
            "cargo".to_string(),
            "+stable".to_string(),
            "check".to_string(),
        ];
        let cargo_test = vec!["cargo".to_string(), "test".to_string()];
        let missing =
            "error: extern location for serde does not exist: target/debug/deps/libserde-abc.rmeta";
        assert!(needs_dependency_metadata_warmup(&cargo_check, missing));
        assert!(needs_dependency_metadata_warmup(
            &cargo_toolchain_check,
            missing
        ));
        assert!(!needs_dependency_metadata_warmup(&cargo_test, missing));
        assert!(!needs_dependency_metadata_warmup(
            &cargo_check,
            "error: could not write target/debug/deps/libserde.rmeta: No space left on device"
        ));
    }
}
