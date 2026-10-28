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

    let warmup = crate::exec::run_remote(
        remote,
        root,
        subdir,
        warmup_command(root, subdir, &command, &env, &first.server_workspace_root)?,
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

/// Runs Cargo in a disposable source copy, preserving reusable artifacts in a sibling target
/// cache instead of letting Cargo or build scripts mutate the persistent workspace (#1443).
fn warmup_command(
    root: &Path,
    subdir: Option<&str>,
    command: &[String],
    env: &[(String, String)],
    server_workspace_root: &str,
) -> Result<Vec<String>> {
    let server_root = Path::new(server_workspace_root);
    anyhow::ensure!(
        server_root.is_absolute(),
        "gateway did not report an absolute workspace root for metadata warm-up"
    );
    let server_cwd = server_root.join(subdir.unwrap_or_default());
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

    for index in 0..cargo_command.len() {
        if cargo_command[index] == "--manifest-path" && index + 1 < cargo_command.len() {
            cargo_command[index + 1] =
                copied_argument_path(&server_root, &server_cwd, &cargo_command[index + 1])?;
        } else if let Some(manifest) = cargo_command[index].strip_prefix("--manifest-path=") {
            cargo_command[index] = format!(
                "--manifest-path={}",
                copied_argument_path(&server_root, &server_cwd, manifest)?
            );
        }
        if cargo_command[index] == "--target-dir" && index + 1 < cargo_command.len() {
            cargo_command[index + 1] =
                absolute_from_workspace(&server_cwd, &cargo_command[index + 1]);
        } else if let Some(target) = cargo_command[index].strip_prefix("--target-dir=") {
            cargo_command[index] = format!(
                "--target-dir={}",
                absolute_from_workspace(&server_cwd, target)
            );
        }
    }

    let has_target_override = env.iter().any(|(name, _)| name == "CARGO_TARGET_DIR")
        || command
            .iter()
            .any(|arg| arg == "--target-dir" || arg.starts_with("--target-dir="));
    let mut out: Vec<String> = [
        "sh",
        "-c",
        "set -eu
         workspace=$1
         relative=$2
         set_target_dir=$3
         shift 3
         workspace=$(cd \"$workspace\" && pwd -P)
         current=$(pwd -P)
         expected=\"$workspace${relative:+/$relative}\"
         expected=$(cd \"$expected\" && pwd -P)
         if [ \"$current\" != \"$expected\" ]; then
           echo 'warm-up directory does not match the gateway workspace and subdirectory' >&2
           exit 2
         fi
         if [ -n \"${CARGO_TARGET_DIR:-}\" ]; then
           case \"$CARGO_TARGET_DIR\" in
             /*) ;;
             *) CARGO_TARGET_DIR=\"$current/$CARGO_TARGET_DIR\"; export CARGO_TARGET_DIR ;;
           esac
         elif [ \"$set_target_dir\" = yes ]; then
           CARGO_TARGET_DIR=\"$workspace/target\"; export CARGO_TARGET_DIR
         fi
         scratch=$(mktemp -d \"${TMPDIR:-/tmp}/prod-code-shadow-warmup.XXXXXX\")
         trap 'rm -rf \"$scratch\"' EXIT HUP INT TERM
         tar -C \"$workspace\" --exclude=./target --exclude=./.git -cf \"$scratch/source.tar\" .
         mkdir \"$scratch/source\"
         tar -xf \"$scratch/source.tar\" -C \"$scratch/source\"
         rm -f \"$scratch/source.tar\"
         cd \"$scratch/source${relative:+/$relative}\"
         \"$@\"",
        "sh",
    ]
    .map(str::to_string)
    .to_vec();
    out.push(server_workspace_root.to_string());
    out.push(subdir.unwrap_or_default().to_string());
    out.push(if has_target_override { "no" } else { "yes" }.to_string());
    out.extend(cargo_command);
    Ok(out)
}

fn copied_argument_path(workspace_root: &Path, cwd: &Path, path: &str) -> Result<String> {
    let path = Path::new(path);
    if !path.is_absolute() {
        return Ok(path.to_string_lossy().into_owned());
    }
    let relative = path.strip_prefix(workspace_root).with_context(|| {
        format!(
            "cannot safely isolate metadata warm-up with manifest path {} outside the workspace",
            path.display()
        )
    })?;
    relative_path(cwd, &workspace_root.join(relative))
        .map(|path| path.to_string_lossy().into_owned())
}

fn absolute_from_workspace(cwd: &Path, path: &str) -> String {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_string_lossy().into_owned()
    } else {
        cwd.join(path).to_string_lossy().into_owned()
    }
}

fn relative_path(from: &Path, to: &Path) -> Result<PathBuf> {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    anyhow::ensure!(
        from.first() == to.first(),
        "cannot map an absolute Cargo path into the isolated workspace"
    );
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut relative = PathBuf::new();
    for component in &from[common..] {
        if matches!(component, std::path::Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    Ok(relative)
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
mod tests;
