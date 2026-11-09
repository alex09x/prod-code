/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Probe child processes without changing the environment shared by parallel tests.
pub fn go_tools_available() -> bool {
    for program in ["go", "gopls"] {
        match std::process::Command::new(program).arg("version").output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => {
                eprintln!(
                    "SKIPPED live Go test: {program} version failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
                return false;
            }
            Err(error) => {
                eprintln!("SKIPPED live Go test: {program} is unavailable: {error}");
                return false;
            }
        }
    }
    true
}

/// Build a workspace binary from inside a live integration test, preferring Mitts when
/// present and uninstrumented while scrubbing crate-scoped test-runner environment variables
/// (`CARGO_PKG_*`, `CARGO_MANIFEST_*`, `OUT_DIR`, `LD_LIBRARY_PATH`) so nested Mitts builds
/// hit the shared CAS and never inherit the enclosing test crate's package identity.
#[allow(dead_code)]
pub fn workspace_build_command(
    workspace: &std::path::Path,
    target_dir: &std::path::Path,
    package: &str,
    bin: &str,
) -> std::process::Command {
    let use_mitts = std::env::var_os("LLVM_PROFILE_FILE").is_none()
        && std::process::Command::new("mitts")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
    let mut cmd = std::process::Command::new(if use_mitts { "mitts" } else { "cargo" });
    if use_mitts {
        for (key, _) in std::env::vars_os() {
            let Some(name) = key.to_str() else {
                continue;
            };
            let keep_cargo = name == "CARGO_HOME"
                || name == "CARGO_INCREMENTAL"
                || name.starts_with("CARGO_PROFILE_")
                || name.starts_with("CARGO_TERM_")
                || name.starts_with("CARGO_NET_")
                || name.starts_with("CARGO_REGISTRIES_")
                || name.starts_with("CARGO_HTTP_");
            if (name == "CARGO" || (name.starts_with("CARGO_") && !keep_cargo))
                || matches!(
                    name,
                    "OUT_DIR"
                        | "LD_LIBRARY_PATH"
                        | "DYLD_LIBRARY_PATH"
                        | "DYLD_FALLBACK_LIBRARY_PATH"
                )
            {
                cmd.env_remove(&key);
            }
        }
    }
    cmd.args([
        "build",
        "--locked",
        "-p",
        package,
        "--bin",
        bin,
        "--target-dir",
    ])
    .arg(target_dir)
    .current_dir(workspace);
    cmd
}
