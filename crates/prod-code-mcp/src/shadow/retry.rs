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
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

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

    let target_dir = metadata_target_dir(&first.server_workspace_root)?;
    let retry_command = command_with_target_dir(&command, &target_dir);
    let retry_env = environment_with_target_dir(&env, &target_dir);
    let warmup = crate::exec::run_remote(
        remote,
        root,
        subdir,
        warmup_command(root, subdir, &retry_command),
        retry_env.clone(),
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
        retry_command,
        retry_env,
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

/// Runs Cargo in a disposable source copy, preserving reusable artifacts in a sibling target
/// cache instead of letting Cargo or build scripts mutate the persistent workspace (#1443).
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
    let mut cargo_command = command.to_vec();
    if locked
        && !cargo_command
            .iter()
            .any(|arg| arg == "--locked" || arg == "--frozen")
    {
        cargo_command.insert(check + 1, "--locked".to_string());
    }
    let mut out: Vec<String> = [
        "sh",
        "-c",
        "set -eu
         manifest=$(cargo locate-project --workspace --message-format plain)
         workspace=${manifest%/Cargo.toml}
         workspace=$(cd \"$workspace\" && pwd -P)
         current=$(pwd -P)
         case \"$current\" in
           \"$workspace\") relative= ;;
           \"$workspace\"/*) relative=\"${current#\"$workspace\"}\" ;;
           *) echo 'warm-up directory is outside the Cargo workspace' >&2; exit 2 ;;
         esac
         scratch=$(mktemp -d \"${TMPDIR:-/tmp}/prod-code-shadow-warmup.XXXXXX\")
         trap 'rm -rf \"$scratch\"' EXIT HUP INT TERM
         tar -C \"$workspace\" --exclude=./target --exclude=./.git -cf \"$scratch/source.tar\" .
         mkdir \"$scratch/source\"
         tar -xf \"$scratch/source.tar\" -C \"$scratch/source\"
         rm -f \"$scratch/source.tar\"
         cd \"$scratch/source$relative\"
         \"$@\"",
        "sh",
    ]
    .map(str::to_string)
    .to_vec();
    out.extend(cargo_command);
    out
}

fn metadata_target_dir(server_workspace_root: &str) -> Result<PathBuf> {
    let workspace = Path::new(server_workspace_root);
    anyhow::ensure!(
        workspace.is_absolute(),
        "gateway did not report an absolute workspace root for metadata warm-up"
    );
    let workspaces_dir = workspace
        .parent()
        .context("gateway workspace root has no parent directory")?;
    let storage_root = workspaces_dir.parent().unwrap_or(workspaces_dir);
    let name = workspace
        .file_name()
        .context("gateway workspace root has no directory name")?;
    Ok(storage_root.join(".prod-code-shadow-targets").join(name))
}

fn command_with_target_dir(command: &[String], target_dir: &Path) -> Vec<String> {
    let target_dir = target_dir.to_string_lossy().into_owned();
    let mut out = command.to_vec();
    if let Some(index) = out.iter().position(|arg| arg == "--target-dir")
        && index + 1 < out.len()
    {
        out[index + 1] = target_dir;
        return out;
    }
    if let Some(arg) = out.iter_mut().find(|arg| arg.starts_with("--target-dir=")) {
        *arg = format!("--target-dir={target_dir}");
        return out;
    }
    let check = out
        .iter()
        .position(|arg| arg == "check")
        .unwrap_or(out.len().saturating_sub(1));
    out.splice(
        check + 1..check + 1,
        ["--target-dir".to_string(), target_dir],
    );
    out
}

fn environment_with_target_dir(
    env: &[(String, String)],
    target_dir: &Path,
) -> Vec<(String, String)> {
    let target_dir = target_dir.to_string_lossy().into_owned();
    let mut out = env.to_vec();
    if let Some((_, value)) = out.iter_mut().find(|(name, _)| name == "CARGO_TARGET_DIR") {
        *value = target_dir;
    } else {
        out.push(("CARGO_TARGET_DIR".to_string(), target_dir));
    }
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

    #[test]
    fn metadata_target_cache_is_outside_the_workspace_tree() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("storage/workspaces/project");
        let target = metadata_target_dir(workspace.to_str().unwrap()).unwrap();

        assert_eq!(
            target,
            temp.path()
                .join("storage/.prod-code-shadow-targets/project")
        );
        assert!(!target.starts_with(&workspace));
    }

    /// #1443: the warm-up runs on a disposable source copy and keeps Cargo output in a separate
    /// reusable target cache.
    #[test]
    fn the_warmup_isolates_build_script_changes_on_success_and_failure() {
        let check = ["cargo", "+stable", "check", "--workspace"].map(str::to_string);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("persistent-workspace");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"warmup_fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
        )
        .unwrap();
        std::fs::write(root.join("src/lib.rs"), "pub fn ready() {}\n").unwrap();
        std::fs::write(
            root.join("build.rs"),
            "fn main() {\n    let dir = std::path::PathBuf::from(std::env::var_os(\"CARGO_MANIFEST_DIR\").unwrap());\n    std::fs::write(dir.join(\"build-script-side-effect.txt\"), \"written\").unwrap();\n    if std::env::var_os(\"FAIL_BUILD\").is_some() { panic!(\"controlled build failure\"); }\n}\n",
        )
        .unwrap();

        let target_dir = temp.path().join("reusable-target");
        let command = command_with_target_dir(&["cargo".into(), "check".into()], &target_dir);
        assert!(command.contains(&target_dir.to_string_lossy().into_owned()));
        let warmup = warmup_command(&root, None, &command);

        let success = std::process::Command::new(&warmup[0])
            .args(&warmup[1..])
            .current_dir(&root)
            .env("CARGO_TARGET_DIR", &target_dir)
            .output()
            .unwrap();
        assert!(
            success.status.success(),
            "{}",
            String::from_utf8_lossy(&success.stderr)
        );
        assert!(!root.join("Cargo.lock").exists());
        assert!(!root.join("build-script-side-effect.txt").exists());
        assert!(target_dir.join("debug").exists());

        let failing_root = temp.path().join("failing-workspace");
        std::fs::create_dir_all(failing_root.join("src")).unwrap();
        std::fs::write(
            failing_root.join("Cargo.toml"),
            "[package]\nname = \"warmup_failure\"\nversion = \"0.1.0\"\nedition = \"2021\"\nbuild = \"build.rs\"\n",
        )
        .unwrap();
        std::fs::write(failing_root.join("src/lib.rs"), "pub fn ready() {}\n").unwrap();
        std::fs::copy(root.join("build.rs"), failing_root.join("build.rs")).unwrap();
        let failing = warmup_command(&failing_root, None, &command);
        let failure = std::process::Command::new(&failing[0])
            .args(&failing[1..])
            .current_dir(&failing_root)
            .env("CARGO_TARGET_DIR", &target_dir)
            .env("FAIL_BUILD", "1")
            .output()
            .unwrap();
        assert!(!failure.status.success());
        assert!(!failing_root.join("Cargo.lock").exists());
        assert!(!failing_root.join("build-script-side-effect.txt").exists());

        // A lock at the root still makes the warm-up invocation immutable.
        std::fs::write(root.join("Cargo.lock"), "").unwrap();
        assert!(
            warmup_command(&root, Some("src"), &check)
                .iter()
                .any(|arg| arg == "--locked")
        );
        let frozen = ["cargo", "check", "--frozen"].map(str::to_string);
        assert!(
            warmup_command(&root, None, &frozen)
                .iter()
                .any(|arg| arg == "--frozen")
        );
    }
}
