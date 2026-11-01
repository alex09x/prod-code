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
    configure_cmake(workspace_root, Path::new("cmake"), Duration::from_secs(180)).await;
}

/// How a CMake configure ended.
#[derive(Debug, PartialEq)]
enum CmakeRun {
    /// Nothing to configure: no `CMakeLists.txt`, or a compilation database already there.
    Skipped,
    Configured,
    /// CMake ran and failed; the last line of its error output.
    Failed(String),
    NotRunnable(String),
    TimedOut,
}

/// [`warm_cmake_compile_commands`] with the program to run and how long to wait for it.
async fn configure_cmake(workspace_root: &Path, program: &Path, limit: Duration) -> CmakeRun {
    if !workspace_root.join("CMakeLists.txt").is_file()
        || workspace_root
            .join("build")
            .join("compile_commands.json")
            .is_file()
    {
        return CmakeRun::Skipped;
    }
    let started = std::time::Instant::now();
    let mut cmd = tokio::process::Command::new(program);
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
    let result = tokio::time::timeout(limit, cmd.output()).await;
    match result {
        Ok(Ok(out)) if out.status.success() => {
            tracing::info!(
                workspace = ?workspace_root,
                duration_ms = started.elapsed().as_millis() as u64,
                "cmake configured build/compile_commands.json for clangd"
            );
            CmakeRun::Configured
        }
        Ok(Ok(out)) => {
            let stderr = String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("")
                .to_string();
            tracing::warn!(
                workspace = ?workspace_root,
                stderr = %stderr,
                "cmake configure failed; clangd runs without a compilation database"
            );
            CmakeRun::Failed(stderr)
        }
        Ok(Err(e)) => {
            tracing::warn!(workspace = ?workspace_root, error = %e, "cmake not runnable");
            CmakeRun::NotRunnable(e.to_string())
        }
        Err(_) => {
            tracing::warn!(workspace = ?workspace_root, "cmake configure timed out");
            CmakeRun::TimedOut
        }
    }
}

/// How long a SwiftPM workspace's load waits for sourcekit-lsp to have the package's build
/// settings. Keep bounded below the client's 10s LSP query timeout (#1766).
pub(crate) const SWIFT_SETTINGS_WAIT: Duration = Duration::from_secs(5);

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
    wait_for_swift_settings_within(engine, root, SWIFT_SETTINGS_WAIT).await;
}

/// [`wait_for_swift_build_settings`] with how long to wait for the settings. `None` when the root
/// has no settings to wait for; otherwise what the check came to, with the reason when the
/// server never reported on the probe.
async fn wait_for_swift_settings_within(
    engine: &prod_code_engine_generic::GenericLspEngine,
    root: &Path,
    wait: Duration,
) -> Option<Result<bool, String>> {
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
        return None;
    }
    let (path, text) = swift_probe_file(root)?;
    let (probe, line) = with_probe_line(&text);
    let started = std::time::Instant::now();
    let checked = engine
        .wait_for_semantic_check(&path, "swift", &probe, line, wait)
        .await;
    let waited_ms = started.elapsed().as_millis() as u64;
    match checked {
        Ok(true) => {
            tracing::info!(workspace = ?root, waited_ms, "sourcekit-lsp has the package's build settings")
        }
        Ok(false) => {
            tracing::warn!(workspace = ?root, waited_ms, "sourcekit-lsp found no type error in the probe; its checks may report syntax errors only")
        }
        Err(ref err) => {
            tracing::warn!(workspace = ?root, waited_ms, error = %err, "sourcekit-lsp never reported on the probe; whether its checks have the package's build settings is unknown")
        }
    }
    Some(checked.map_err(|err| err.to_string()))
}

/// A Swift source of the SwiftPM package at `root`, and its text: prefers `.swift` files inside
/// target directories declared in `Package.swift` (via `path: "…"` or default `Sources/<name>` /
/// `Tests/<name>`, excluding `exclude: […]` entries) (#1766, #1500), falling back to `Sources/`
/// when `Package.swift` lists no targets.
pub(crate) fn swift_probe_file(root: &Path) -> Option<(PathBuf, String)> {
    let manifest = std::fs::read_to_string(root.join("Package.swift")).ok()?;
    let declared = declared_swift_targets(&manifest, root);
    if !declared.is_empty()
        && let Some(found) = declared
            .iter()
            .find_map(|(dir, excludes)| first_swift_file_excluding(dir, excludes))
    {
        return Some(found);
    }
    std::iter::once(root.join("Sources"))
        .chain(declared_target_paths(&manifest).map(|rel| root.join(rel)))
        .find_map(|dir| first_swift_file(&dir))
}

fn is_safe_relative_path(rel: &str) -> bool {
    !rel.is_empty()
        && !rel.starts_with('/')
        && !Path::new(rel)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

fn extract_quoted_arg<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let (_, after) = block.split_once(key)?;
    let rest = after.trim_start().strip_prefix('"')?;
    let val = &rest[..rest.find('"')?];
    Some(val)
}

fn extract_excluded_paths(block: &str, dir: &Path) -> Vec<PathBuf> {
    let Some((_, after)) = block.split_once("exclude:") else {
        return Vec::new();
    };
    let Some(start) = after.find('[') else {
        return Vec::new();
    };
    let rest = &after[start + 1..];
    let Some(end) = rest.find(']') else {
        return Vec::new();
    };
    let list = &rest[..end];
    let mut out = Vec::new();
    let mut rem = list;
    while let Some(q1) = rem.find('"') {
        let after_q1 = &rem[q1 + 1..];
        let Some(q2) = after_q1.find('"') else {
            break;
        };
        let item = &after_q1[..q2];
        if is_safe_relative_path(item) {
            out.push(dir.join(item));
        }
        rem = &after_q1[q2 + 1..];
    }
    out
}

/// Target directories and excluded subpaths declared in `Package.swift` (#1766).
fn declared_swift_targets(manifest: &str, root: &Path) -> Vec<(PathBuf, Vec<PathBuf>)> {
    let mut primary = Vec::new();
    let mut tests = Vec::new();
    for (kind, is_test) in [
        (".executableTarget(", false),
        (".target(", false),
        (".macro(", false),
        (".testTarget(", true),
    ] {
        for (at, _) in manifest.match_indices(kind) {
            let tail = &manifest[at + kind.len()..];
            let mut depth = 1usize;
            let mut end = tail.len();
            for (idx, ch) in tail.char_indices() {
                match ch {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = idx;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let block = &tail[..end];
            let dir = if let Some(rel) = extract_quoted_arg(block, "path:") {
                if !is_safe_relative_path(rel) {
                    continue;
                }
                root.join(rel)
            } else if let Some(name) = extract_quoted_arg(block, "name:") {
                if !is_safe_relative_path(name) {
                    continue;
                }
                let base = if is_test { "Tests" } else { "Sources" };
                let candidate = root.join(base).join(name);
                if candidate.is_dir() {
                    candidate
                } else {
                    root.join(base)
                }
            } else {
                continue;
            };
            let excludes = extract_excluded_paths(block, &dir);
            if is_test {
                tests.push((dir, excludes));
            } else {
                primary.push((dir, excludes));
            }
        }
    }
    primary.extend(tests);
    primary
}

/// The directories a SwiftPM manifest gives its targets with `path: "…"`, in order: relative
/// ones inside the package only.
fn declared_target_paths(manifest: &str) -> impl Iterator<Item = &str> {
    manifest.match_indices("path:").filter_map(|(at, _)| {
        let rest = manifest[at + "path:".len()..]
            .trim_start()
            .strip_prefix('"')?;
        let rel = &rest[..rest.find('"')?];
        is_safe_relative_path(rel).then_some(rel)
    })
}

/// The first `.swift` file under `dir` by path, and its text.
fn first_swift_file(dir: &Path) -> Option<(PathBuf, String)> {
    first_swift_file_excluding(dir, &[])
}

fn first_swift_file_excluding(dir: &Path, excludes: &[PathBuf]) -> Option<(PathBuf, String)> {
    let mut pending = vec![dir.to_path_buf()];
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
            if excludes.iter().any(|ex| path.starts_with(ex)) {
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

#[cfg(test)]
mod tests;
#[cfg(test)]
mod wait_tests;
