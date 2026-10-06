/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Host toolchain and compiler inventory discovery.
//!
//! Gathers installed language compilers, runtime binaries, and language servers
//! with their versions. Executed infrequently (e.g. at startup and during periodic
//! janitor maintenance) so that expensive version checks never run on the request
//! path or during frequent telemetry snapshots.

use prod_code_protocol::{EngineToolchainInfo, ToolchainInventory, ToolchainVersion};
use std::process::Command;
use std::time::{Duration, Instant};

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Known tools to probe per language engine.
const ENGINE_TOOLS: &[(&str, &[(&str, &[&str])])] = &[
    (
        "rust",
        &[("rustc", &["--version"]), ("cargo", &["--version"])],
    ),
    ("go", &[("go", &["version"]), ("gopls", &["version"])]),
    (
        "cpp",
        &[("clang", &["--version"]), ("clangd", &["--version"])],
    ),
    (
        "swift",
        &[("swift", &["--version"]), ("sourcekit-lsp", &["--version"])],
    ),
    (
        "python",
        &[
            ("python3", &["--version"]),
            ("basedpyright", &["--version"]),
            ("pyright", &["--version"]),
        ],
    ),
    (
        "typescript",
        &[
            ("node", &["--version"]),
            ("tsc", &["--version"]),
            ("typescript-language-server", &["--version"]),
        ],
    ),
    ("zig", &[("zig", &["version"]), ("zls", &["--version"])]),
    (
        "java",
        &[("javac", &["--version"]), ("java", &["--version"])],
    ),
    ("kotlin", &[("kotlinc", &["-version"])]),
    ("dart", &[("dart", &["--version"])]),
    ("csharp", &[("dotnet", &["--version"])]),
    ("elixir", &[("elixir", &["--version"])]),
    (
        "scala",
        &[("scala", &["-version"]), ("metals", &["--version"])],
    ),
    ("lua", &[("lua-language-server", &["--version"])]),
    ("shell", &[("shellcheck", &["--version"])]),
    ("protobuf", &[("protoc", &["--version"])]),
];

/// Collects toolchain inventory across the host.
///
/// Blocking: execute only inside `spawn_blocking` or background tasks.
pub fn collect_toolchain_inventory(
    node: &str,
    advertised_engines: &[String],
) -> ToolchainInventory {
    let mut engines = Vec::new();

    for &(engine, tools) in ENGINE_TOOLS {
        let is_advertised = advertised_engines.iter().any(|e| {
            let base = e.split_whitespace().next().unwrap_or(e);
            base.eq_ignore_ascii_case(engine)
        });

        let mut toolchain_versions = Vec::new();
        for &(tool, args) in tools {
            if let Some(ver) = probe_tool_version(tool, args) {
                toolchain_versions.push(ToolchainVersion {
                    tool: tool.to_string(),
                    version: ver,
                });
            }
        }

        let available = is_advertised || !toolchain_versions.is_empty();
        if available || !toolchain_versions.is_empty() {
            engines.push(EngineToolchainInfo {
                engine: engine.to_string(),
                available,
                toolchains: toolchain_versions,
                details: None,
            });
        }
    }

    ToolchainInventory {
        ts_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
        node: node.to_string(),
        engines,
    }
}

/// Executes a tool binary with version flags and parses its first output line.
fn probe_tool_version(tool: &str, args: &[&str]) -> Option<String> {
    if prod_code_engine_generic::which_bin(tool).is_err() {
        return None;
    }

    let start = Instant::now();
    let mut child = Command::new(tool)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // Bounded wait loop
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    let output = child.wait_with_output().ok()?;
                    let stdout_str = String::from_utf8_lossy(&output.stdout);
                    if let Some(line) = stdout_str.lines().find(|l| !l.trim().is_empty()) {
                        return Some(clean_version_line(line.trim()));
                    }
                    let stderr_str = String::from_utf8_lossy(&output.stderr);
                    if let Some(line) = stderr_str.lines().find(|l| !l.trim().is_empty()) {
                        return Some(clean_version_line(line.trim()));
                    }
                    return None;
                }
                return None;
            }
            Ok(None) => {
                if start.elapsed() >= VERSION_PROBE_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => return None,
        }
    }
}

/// Normalizes output from various `--version` commands into concise version strings.
fn clean_version_line(raw: &str) -> String {
    // Truncate overly verbose output (e.g. clang full license headers) to max 80 chars
    let s = if raw.len() > 80 { &raw[..80] } else { raw };
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_version_line_truncates_and_trims() {
        assert_eq!(
            clean_version_line("  rustc 1.97.1 (c980f4866)  "),
            "rustc 1.97.1 (c980f4866)"
        );
        let long = "x".repeat(120);
        let cleaned = clean_version_line(&long);
        assert_eq!(cleaned.len(), 80);
    }

    #[test]
    fn probes_known_system_tool_if_available() {
        if prod_code_engine_generic::which_bin("cargo").is_ok() {
            let ver = probe_tool_version("cargo", &["--version"]);
            assert!(ver.is_some());
            assert!(ver.unwrap().contains("cargo"));
        }
    }
}
