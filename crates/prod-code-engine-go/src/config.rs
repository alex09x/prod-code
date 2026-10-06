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
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_HEALTH_PROBE_INTERVAL: Duration = Duration::from_secs(60);
pub const MAX_IDLE_PROBE_TIMEOUTS: usize = 3;
pub const HEALTH_PROBE_METHOD: &str = "prodCode/healthProbe";
pub const HEALTH_PROBE_ID_PREFIX: &str = "prod-code-health:";

/// Configuration for the managed Go engine.
#[derive(Debug, Clone)]
pub struct GoConfig {
    /// Optional explicit path to the `gopls` binary.
    pub gopls_path: Option<PathBuf>,
    /// Shared cache directory for `GOCACHE` and `GOMODCACHE`.
    pub shared_cache_dir: Option<PathBuf>,
    /// Additional environment variables for the Go toolchain.
    pub extra_env: HashMap<String, String>,
    /// Build flags / tags (e.g. `["-tags=integration"]`).
    pub build_flags: Vec<String>,
    /// How often an idle initialized server is asked a private dispatch-only probe. `None`
    /// disables probes; the default is deliberately conservative for normal workspaces.
    pub health_probe_interval: Option<Duration>,
}

impl Default for GoConfig {
    fn default() -> Self {
        Self {
            gopls_path: None,
            shared_cache_dir: None,
            extra_env: HashMap::new(),
            build_flags: Vec::new(),
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }
}

/// Discovers the `gopls` executable on the host system.
pub fn find_gopls_binary(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = explicit.filter(|p| p.exists()) {
        return Some(path.to_path_buf());
    }

    // Check $PATH via which
    if let Ok(path) = which_gopls() {
        return Some(path);
    }

    // Standard Go installation locations
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("go/bin/gopls"));
        candidates.push(PathBuf::from(&home).join(".local/bin/gopls"));
    }
    if let Ok(gopath) = std::env::var("GOPATH") {
        candidates.push(PathBuf::from(gopath).join("bin/gopls"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/gopls"));
    candidates.push(PathBuf::from("/snap/bin/gopls"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/gopls"));

    candidates.into_iter().find(|p| p.exists())
}

fn which_gopls() -> Result<PathBuf> {
    let output = std::process::Command::new("which")
        .arg("gopls")
        .output()
        .context("which command failed")?;
    if output.status.success() {
        let path_str = String::from_utf8(output.stdout)?.trim().to_string();
        if !path_str.is_empty() {
            return Ok(PathBuf::from(path_str));
        }
    }
    anyhow::bail!("gopls not found in PATH")
}
