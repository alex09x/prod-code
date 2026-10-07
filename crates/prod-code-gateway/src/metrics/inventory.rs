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
use std::collections::HashSet;
use std::process::Command;
use std::time::{Duration, Instant};

const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

type ToolProbe = (&'static str, &'static [&'static str]);
type EngineToolProbes = &'static [ToolProbe];
type EngineProbeTable = &'static [(&'static str, EngineToolProbes)];

/// Known tools to probe per language engine.
const ENGINE_TOOLS: EngineProbeTable = &[
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

fn normalize_advertised_engine(engine: &str) -> Option<String> {
    engine
        .split_whitespace()
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase)
}

fn collect_engine_inventory(
    advertised_engines: &[String],
    engine_tools: EngineProbeTable,
    mut probe: impl FnMut(&str, &[&str]) -> Option<String>,
) -> Vec<EngineToolchainInfo> {
    let advertised: HashSet<String> = advertised_engines
        .iter()
        .filter_map(|engine| normalize_advertised_engine(engine))
        .collect();
    let mut seen = HashSet::new();
    let mut engines = Vec::new();

    for &(engine, tools) in engine_tools {
        let engine = engine.to_ascii_lowercase();
        let is_advertised = advertised.contains(&engine);
        let mut toolchains = Vec::new();
        for &(tool, args) in tools {
            if let Some(version) = probe(tool, args) {
                toolchains.push(ToolchainVersion {
                    tool: tool.to_string(),
                    version,
                });
            }
        }

        let available = is_advertised;
        if available || !toolchains.is_empty() {
            seen.insert(engine.clone());
            engines.push(EngineToolchainInfo {
                engine,
                available,
                toolchains,
                details: None,
            });
        }
    }

    let mut unprobed: Vec<String> = advertised.difference(&seen).cloned().collect();
    unprobed.sort();
    for engine in unprobed {
        engines.push(EngineToolchainInfo {
            engine,
            available: true,
            toolchains: Vec::new(),
            details: None,
        });
    }

    engines
}

/// Collects toolchain inventory across the host.
///
/// Blocking: execute only inside `spawn_blocking` or background tasks.
pub fn collect_toolchain_inventory(
    node: &str,
    advertised_engines: &[String],
) -> ToolchainInventory {
    let engines = collect_engine_inventory(advertised_engines, ENGINE_TOOLS, probe_tool_version);

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

    #[test]
    fn metrics_review_probe_does_not_make_engine_available() {
        let inventory = collect_engine_inventory(&[], ENGINE_TOOLS, |tool, _| {
            (tool == "python3").then_some("Python 3.13".to_string())
        });
        let python = inventory
            .iter()
            .find(|engine| engine.engine == "python")
            .expect("probed Python runtime remains in inventory");
        assert!(!python.available);
        assert_eq!(python.toolchains.len(), 1);
        assert_eq!(python.toolchains[0].tool, "python3");
    }

    #[test]
    fn metrics_review_advertised_engine_without_probe_is_included() {
        let advertised = vec!["php (php-lsp)".to_string(), "PHP".to_string()];
        let inventory = collect_engine_inventory(&advertised, &[], |_, _| None);
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].engine, "php");
        assert!(inventory[0].available);
        assert!(inventory[0].toolchains.is_empty());
    }
}
