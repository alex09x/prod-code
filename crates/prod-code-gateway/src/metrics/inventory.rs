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

fn collect_engine_inventory<F>(
    advertised_engines: &[String],
    engine_tools: EngineProbeTable,
    probe: F,
) -> Vec<EngineToolchainInfo>
where
    F: Fn(&str, &[&str]) -> Option<String> + Sync,
{
    let advertised: HashSet<String> = advertised_engines
        .iter()
        .filter_map(|engine| normalize_advertised_engine(engine))
        .collect();
    let probed: Vec<(String, bool, Vec<ToolchainVersion>)> = std::thread::scope(|s| {
        let handles: Vec<_> = engine_tools
            .iter()
            .map(|&(engine, tools)| {
                let probe = &probe;
                let engine = engine.to_ascii_lowercase();
                let is_advertised = advertised.contains(&engine);
                s.spawn(move || {
                    let mut toolchains = Vec::new();
                    for &(tool, args) in tools {
                        if let Some(version) = probe(tool, args) {
                            toolchains.push(ToolchainVersion {
                                tool: tool.to_string(),
                                version,
                            });
                        }
                    }
                    (engine, is_advertised, toolchains)
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });

    let mut seen = HashSet::new();
    let mut engines = Vec::new();

    for (engine, is_advertised, toolchains) in probed {
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

    // Both pipes are drained while the tool runs, and read within the same deadline: a tool
    // that fills a pipe no longer times out, and a descendant that keeps a pipe open after the
    // tool exits no longer holds the inventory.
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        let (sent, got) = std::sync::mpsc::channel();
        if let Some(mut pipe) = pipe {
            std::thread::spawn(move || {
                let mut text = Vec::new();
                let _ = pipe.read_to_end(&mut text);
                let _ = sent.send(text);
            });
        }
        got
    };
    use std::io::Read as _;
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as _));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as _));
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) if start.elapsed() < VERSION_PROBE_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(15));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !succeeded {
        return None;
    }
    let left = || VERSION_PROBE_TIMEOUT.saturating_sub(start.elapsed());
    for got in [stdout, stderr] {
        let text = got.recv_timeout(left()).unwrap_or_default();
        let text = String::from_utf8_lossy(&text);
        if let Some(line) = text.lines().find(|l| !l.trim().is_empty()) {
            return Some(clean_version_line(line.trim()));
        }
    }
    None
}

/// Normalizes output from various `--version` commands into concise version strings.
fn clean_version_line(raw: &str) -> String {
    // Truncate overly verbose output (e.g. clang full license headers) to max 80 chars
    let mut cut = raw.len().min(80);
    while !raw.is_char_boundary(cut) {
        cut -= 1;
    }
    let s = &raw[..cut];
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
