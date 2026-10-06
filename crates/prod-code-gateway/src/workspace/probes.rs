/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Configures a CMake project into `build/` with `compile_commands.json` before clangd starts,
/// so its background index covers the whole tree (cross-file rename and references) from the
/// first query. Best effort: a failure only means clangd runs without a compilation database.
pub(crate) async fn warm_cmake_compile_commands(workspace_root: &Path) {
    if !workspace_root.join("CMakeLists.txt").is_file()
        || workspace_root
            .join("build")
            .join("compile_commands.json")
            .is_file()
    {
        return;
    }
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new("cmake");
    cmd.args([
        "-S",
        ".",
        "-B",
        "build",
        "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON",
    ])
    .current_dir(workspace_root)
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::piped());
    for (k, v) in crate::compiler_cache_env(workspace_root, crate::on_path("ccache")) {
        cmd.env(k, v);
    }
    let result = tokio::time::timeout(Duration::from_secs(180), cmd.output()).await;
    match result {
        Ok(Ok(out)) if out.status.success() => tracing::info!(
            workspace = ?workspace_root,
            duration_ms = started.elapsed().as_millis() as u64,
            "cmake configured build/compile_commands.json for clangd"
        ),
        Ok(Ok(out)) => tracing::warn!(
            workspace = ?workspace_root,
            stderr = %String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""),
            "cmake configure failed; clangd runs without a compilation database"
        ),
        Ok(Err(e)) => tracing::warn!(workspace = ?workspace_root, error = %e, "cmake not runnable"),
        Err(_) => tracing::warn!(workspace = ?workspace_root, "cmake configure timed out"),
    }
}

/// How long a SwiftPM workspace's load waits for sourcekit-lsp to have the package's build
/// settings. Resolving a package's dependencies the first time can take a while.
pub(crate) const SWIFT_SETTINGS_WAIT: Duration = Duration::from_secs(45);

/// The line a probe appends to a file of the package: a declaration only a type check faults.
pub(crate) const SWIFT_PROBE: &str = "let __prodCodeProbe: Int = \"\"";

/// Holds a SwiftPM workspace's load until sourcekit-lsp checks its files with the package's
/// build settings (#295). Until it has loaded them it checks with fallback settings that report
/// syntax errors only, and the first checks after a load said "0 errors" for code that does not
/// compile. A file of the package, with [`SWIFT_PROBE`] appended, is kept open until the error on
/// that line appears. No session can reach the workspace before its load ends, so nothing else
/// sees the probe. A root without `Package.swift` (an Xcode project) has no settings to wait for.
pub(crate) async fn wait_for_swift_build_settings(
    engine: &prod_code_engine_generic::GenericLspEngine,
    root: &Path,
) {
    if root.components().any(|c| {
        matches!(
            c.as_os_str().to_string_lossy().as_ref(),
            "target"
                | "node_modules"
                | "vendor"
                | "build"
                | "dist"
                | ".build"
                | "Pods"
                | "DerivedData"
        )
    }) {
        return;
    }
    let Some((path, text)) = swift_probe_file(root) else {
        return;
    };
    let (probe, line) = with_probe_line(&text);
    let started = std::time::Instant::now();
    let checked = engine
        .wait_for_semantic_check(&path, "swift", &probe, line, SWIFT_SETTINGS_WAIT)
        .await;
    let waited_ms = started.elapsed().as_millis() as u64;
    match checked {
        Ok(true) => {
            tracing::info!(workspace = ?root, waited_ms, "sourcekit-lsp has the package's build settings")
        }
        Ok(false) => {
            tracing::warn!(workspace = ?root, waited_ms, "sourcekit-lsp found no type error in the probe; its checks may report syntax errors only")
        }
        Err(err) => {
            tracing::warn!(workspace = ?root, waited_ms, error = %err, "sourcekit-lsp never reported on the probe; whether its checks have the package's build settings is unknown")
        }
    }
}

/// A Swift source of the SwiftPM package at `root`, and its text: the first under `Sources/` by
/// path, hidden directories (`.build`) aside.
pub(crate) fn swift_probe_file(root: &Path) -> Option<(PathBuf, String)> {
    if !root.join("Package.swift").is_file() {
        return None;
    }
    let mut pending = vec![root.join("Sources")];
    let mut found = Vec::new();
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "swift") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
        .into_iter()
        .find_map(|p| std::fs::read_to_string(&p).ok().map(|t| (p, t)))
}

/// `text` with [`SWIFT_PROBE`] appended on a line of its own, and that line's 0-based number.
pub(crate) fn with_probe_line(text: &str) -> (String, u64) {
    let mut probe = text.to_string();
    if !probe.is_empty() && !probe.ends_with('\n') {
        probe.push('\n');
    }
    let line = probe.matches('\n').count() as u64;
    probe.push_str(SWIFT_PROBE);
    probe.push('\n');
    (probe, line)
}
