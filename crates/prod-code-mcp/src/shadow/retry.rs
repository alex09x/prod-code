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
        warmup_command(root, subdir, &command),
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

/// The warm-up runs in the node's persistent copy of the workspace, not in a shadow, so it must
/// leave that copy as the checkout has it (#1443). With a `Cargo.lock` in the checkout, it runs
/// `--locked` and cannot change it. Without one, Cargo would write one there that the checkout
/// does not have: it is removed again after the warm-up.
fn warmup_command(root: &Path, subdir: Option<&str>, command: &[String]) -> Vec<String> {
    let start = subdir.map_or_else(|| root.to_path_buf(), |s| root.join(s));
    let locked = start
        .ancestors()
        .take_while(|dir| dir.starts_with(root))
        .any(|dir| dir.join("Cargo.lock").is_file());
    let check = command
        .iter()
        .position(|arg| arg == "check")
        .unwrap_or(command.len().saturating_sub(1));
    if locked {
        let mut out = command.to_vec();
        if !out.iter().any(|arg| arg == "--locked" || arg == "--frozen") {
            out.insert(check + 1, "--locked".to_string());
        }
        return out;
    }
    let mut out: Vec<String> = [
        "sh",
        "-c",
        "\"$@\"; status=$?; manifest=$(cargo locate-project --workspace --message-format plain \
         2>/dev/null) && rm -f \"$(dirname \"$manifest\")/Cargo.lock\"; exit $status",
        "sh",
    ]
    .map(str::to_string)
    .to_vec();
    out.extend(command.iter().cloned());
    out
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

    /// #1443: the warm-up cannot leave a lock file the checkout does not have, nor change one
    /// it has.
    #[test]
    fn the_warmup_keeps_the_persistent_copy_as_the_checkout_has_it() {
        let check = ["cargo", "+stable", "check", "--workspace"].map(str::to_string);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::create_dir_all(root.join("crates/a")).unwrap();

        let unlocked = warmup_command(root, Some("crates/a"), &check);
        assert_eq!(&unlocked[..2], &["sh".to_string(), "-c".to_string()]);
        assert!(
            unlocked[2].contains("cargo locate-project --workspace"),
            "{unlocked:?}"
        );
        assert!(unlocked[2].contains("rm -f"), "{unlocked:?}");
        assert!(unlocked[2].contains("exit $status"), "{unlocked:?}");
        assert_eq!(&unlocked[4..], &check);

        // A lock at the workspace root, above the subdirectory.
        std::fs::write(root.join("Cargo.lock"), "").unwrap();
        assert_eq!(
            warmup_command(root, Some("crates/a"), &check),
            ["cargo", "+stable", "check", "--locked", "--workspace"].map(str::to_string)
        );
        let frozen = ["cargo", "check", "--frozen"].map(str::to_string);
        assert_eq!(warmup_command(root, None, &frozen), frozen);
    }
}
