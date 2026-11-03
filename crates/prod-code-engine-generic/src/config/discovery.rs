/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

pub mod probe;
pub mod process;
pub use probe::*;
pub use process::*;

/// How long a successful npm root discovery is cached in memory.
pub const NPM_SUCCESS_CACHE_TTL: Duration = Duration::from_secs(300);

/// How long a failed npm root discovery is cached to prevent tight probe loops and child accumulation.
pub const NPM_FAILURE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Environment variables whose changes invalidate cached npm discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NpmEnvironmentSnapshot {
    pub path: Option<OsString>,
    pub prefix: Option<OsString>,
    pub npm_config_prefix: Option<OsString>,
}

impl NpmEnvironmentSnapshot {
    pub fn capture(env_lookup: impl Fn(&str) -> Option<OsString>) -> Self {
        Self {
            path: env_lookup("PATH"),
            prefix: env_lookup("PREFIX"),
            npm_config_prefix: env_lookup("npm_config_prefix"),
        }
    }
}

/// Cached result of an npm global root probe.
#[derive(Debug, Clone)]
pub struct NpmCacheEntry {
    pub exe_path: Option<PathBuf>,
    pub exe_mtime: Option<SystemTime>,
    pub env_snapshot: NpmEnvironmentSnapshot,
    pub probed_at: Instant,
    pub result: Option<PathBuf>,
}

impl NpmCacheEntry {
    pub fn is_fresh(
        &self,
        current_exe: Option<&Path>,
        current_mtime: Option<SystemTime>,
        current_env: &NpmEnvironmentSnapshot,
        now: Instant,
    ) -> bool {
        if self.exe_path.as_deref() != current_exe {
            return false;
        }
        if self.exe_mtime != current_mtime {
            return false;
        }
        if &self.env_snapshot != current_env {
            return false;
        }
        match &self.result {
            Some(dir) => {
                if !dir.is_dir() {
                    return false;
                }
                now.saturating_duration_since(self.probed_at) < NPM_SUCCESS_CACHE_TTL
            }
            None => now.saturating_duration_since(self.probed_at) < NPM_FAILURE_CACHE_TTL,
        }
    }
}

/// Thread-safe in-memory cache for npm global root discovery.
#[derive(Debug, Default)]
pub struct NpmRootCache {
    entry: RwLock<Option<NpmCacheEntry>>,
}

impl NpmRootCache {
    pub fn new() -> Self {
        Self {
            entry: RwLock::new(None),
        }
    }

    pub fn cached_entry(&self) -> Option<NpmCacheEntry> {
        self.entry.read().ok().and_then(|g| g.clone())
    }
}

/// The native TypeScript 7 compiler binary, which doubles as the language server
/// (`tsc --lsp --stdio`): `tsgo` on PATH, or the platform package under the global
/// `typescript` install (`@typescript/typescript-<os>-<arch>/lib/tsc`).
pub fn native_typescript_lsp() -> Option<PathBuf> {
    native_typescript_lsp_with_resolver(which_bin("tsgo").ok(), npm_global_root)
}

/// [`native_typescript_lsp`] with injected tsgo binary and lazily evaluated global npm root.
pub fn native_typescript_lsp_with_resolver(
    tsgo: Option<PathBuf>,
    npm_root: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if let Some(tsgo) = tsgo {
        return Some(tsgo);
    }
    let root = npm_root()?;
    native_typescript_lsp_with(None, Some(&root))
}

/// [`native_typescript_lsp`] with injected tsgo binary and global npm root.
pub fn native_typescript_lsp_with(
    tsgo: Option<PathBuf>,
    npm_root: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(tsgo) = tsgo {
        return Some(tsgo);
    }
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let candidate = npm_root?
        .join("typescript")
        .join("node_modules")
        .join("@typescript")
        .join(format!("typescript-{os}-{arch}"))
        .join("lib")
        .join("tsc");
    candidate.is_file().then_some(candidate)
}

/// Global npm module root (`npm root -g`), where `npm install -g` puts packages.
///
/// Bounded with timeout, persistent logging disabled (`--logs-max=0`) and npm's log
/// cleanup confined to a private probe directory, exit status and directory existence
/// checked, and cached in memory with revalidation on environment, executable, or
/// filesystem changes (#1794).
pub fn npm_global_root() -> Option<PathBuf> {
    static CACHE: std::sync::OnceLock<NpmRootCache> = std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(NpmRootCache::default);
    npm_global_root_with(
        cache,
        || resolve_npm_binary_with(|key| std::env::var_os(key)),
        |key| std::env::var_os(key),
        DEFAULT_NPM_PROBE_TIMEOUT,
    )
}

/// [`npm_global_root`] with injected cache, executable lookup, environment, and timeout.
pub fn npm_global_root_with(
    cache: &NpmRootCache,
    lookup_npm: impl Fn() -> Option<PathBuf>,
    env_lookup: impl Fn(&str) -> Option<OsString>,
    timeout: Duration,
) -> Option<PathBuf> {
    let current_exe = lookup_npm();
    let current_mtime = current_exe
        .as_ref()
        .and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok());
    let current_env = NpmEnvironmentSnapshot::capture(&env_lookup);

    if let Some(result) = cache
        .entry
        .read()
        .ok()
        .as_deref()
        .and_then(|g| g.as_ref())
        .filter(|e| {
            e.is_fresh(
                current_exe.as_deref(),
                current_mtime,
                &current_env,
                Instant::now(),
            )
        })
        .map(|e| e.result.clone())
    {
        return result;
    }

    let mut guard = cache.entry.write().unwrap_or_else(|e| e.into_inner());
    // Freshness after the lock wait: another caller may have just finished a probe.
    let now = Instant::now();
    if let Some(result) = guard
        .as_ref()
        .filter(|e| e.is_fresh(current_exe.as_deref(), current_mtime, &current_env, now))
        .map(|e| e.result.clone())
    {
        return result;
    }

    let result = if let Some(ref exe) = current_exe {
        let mut extra_env = Vec::new();
        if let Some(p) = env_lookup("PATH") {
            extra_env.push((OsString::from("PATH"), p));
        }
        if let Some(p) = env_lookup("npm_config_prefix") {
            extra_env.push((OsString::from("npm_config_prefix"), p));
        }
        if let Some(p) = env_lookup("PREFIX") {
            extra_env.push((OsString::from("PREFIX"), p));
        }
        probe_npm_root(exe, timeout, extra_env)
    } else {
        None
    };

    *guard = Some(NpmCacheEntry {
        exe_path: current_exe,
        exe_mtime: current_mtime,
        env_snapshot: current_env,
        // Stamped on completion, so the TTL counts from when the answer was observed.
        probed_at: Instant::now(),
        result: result.clone(),
    });

    result
}

/// Resolves `npm` from `PATH` in `lookup`, falling back to `which_bin("npm")`.
///
/// On Unix a file without an execute bit is skipped, as the shell would skip it, so it
/// cannot shadow a real `npm` later on `PATH`.
pub fn resolve_npm_binary_with(lookup: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path_var) = lookup("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("npm");
            if is_executable_file(&candidate) {
                return Some(candidate);
            }
            #[cfg(windows)]
            for ext in ["cmd", "exe", "bat"] {
                let candidate = dir.join(format!("npm.{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    which_bin("npm").ok()
}

/// A regular file (after symlinks) that, on Unix, has at least one execute bit.
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

pub fn settings_for_section(root: &Path, section: &str) -> serde_json::Value {
    let analysis = serde_json::json!({
        "diagnosticMode": "workspace",
        "autoSearchPaths": true,
        "useLibraryCodeForTypes": true,
    });
    match section {
        "python.analysis" | "basedpyright.analysis" => analysis,
        s if s.starts_with("python") || s.starts_with("basedpyright") => {
            let mut settings = serde_json::json!({ "analysis": analysis });
            if let Some(python) = venv_python(root) {
                settings["pythonPath"] = serde_json::Value::String(python);
                settings["venvPath"] =
                    serde_json::Value::String(root.to_string_lossy().into_owned());
                settings["venv"] = serde_json::Value::String(".venv".to_string());
            }
            settings
        }
        _ => serde_json::json!({}),
    }
}

/// `<root>/.venv/bin/python` (or `venv/`, or in an ancestor checkout) when the project carries a virtual environment.
pub fn venv_python(root: &Path) -> Option<String> {
    for ancestor in root.ancestors() {
        for dir in [".venv", "venv"] {
            let candidate = ancestor.join(dir).join("bin").join("python");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// Helper to check if an executable binary is present in PATH.
pub fn which_bin(name: &str) -> Result<PathBuf> {
    let output = std::process::Command::new("which")
        .arg(name)
        .output()
        .context("which command failed")?;
    if output.status.success() {
        let path_str = String::from_utf8(output.stdout)?.trim().to_string();
        if !path_str.is_empty() {
            return Ok(PathBuf::from(path_str));
        }
    }
    anyhow::bail!("Binary {name} not found in PATH")
}

#[cfg(test)]
mod tests;
