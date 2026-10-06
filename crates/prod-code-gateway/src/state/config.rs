/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Configuration file support (`gateway.toml`) for prod-code gateway daemon.
//!
//! Provides structured TOML configuration for server binding, storage, metrics
//! collection, bounded retention, and Prometheus pull/push export.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Top-level gateway configuration file schema.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct GatewayConfigFile {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
}

/// Server network and operational parameters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServerConfig {
    #[serde(default)]
    pub bind: Option<SocketAddr>,
    #[serde(default)]
    pub socket_path: Option<PathBuf>,
    #[serde(default)]
    pub storage: Option<PathBuf>,
    #[serde(default)]
    pub advertise: Option<String>,
    #[serde(default)]
    pub peers: Option<String>,
    #[serde(default)]
    pub engines: Option<Vec<String>>,
    #[serde(default)]
    pub idle_evict_secs: Option<u64>,
    #[serde(default)]
    pub engine_reserve_mib: Option<u64>,
    #[serde(default)]
    pub max_concurrent_engine_loads: Option<usize>,
    #[serde(default)]
    pub prune_worktree_secs: Option<u64>,
    #[serde(default)]
    pub prune_workspace_secs: Option<u64>,
    #[serde(default)]
    pub prune_below_free_percent: Option<u64>,
    #[serde(default)]
    pub build_cache_ram: Option<bool>,
    #[serde(default)]
    pub build_cache_dir: Option<PathBuf>,
}

/// Metrics collection and export settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsConfig {
    /// Whether metrics ring collection and persistent logging is enabled.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Telemetry snapshot interval in seconds (default: 30).
    #[serde(default)]
    pub snapshot_interval_secs: Option<u64>,
    /// Toolchain inventory interval in seconds (default: 3600).
    #[serde(default)]
    pub inventory_interval_secs: Option<u64>,
    #[serde(default)]
    pub retention: Option<MetricsRetentionConfig>,
    #[serde(default)]
    pub prometheus: Option<PrometheusConfig>,
}

/// Bounded metrics log retention policy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetricsRetentionConfig {
    /// Retention in days for stored metrics files (default: 14).
    #[serde(default)]
    pub retention_days: Option<u64>,
    /// Maximum storage bytes budget for stored metrics files (default: 500 MB).
    #[serde(default)]
    pub max_storage_bytes: Option<u64>,
}

/// Prometheus HTTP scrape and Pushgateway configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrometheusConfig {
    /// Enable or disable Prometheus exporter/pusher.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Export mode: "scrape" (pull HTTP server), "push" (push to Pushgateway), "both", or "disabled".
    #[serde(default)]
    pub mode: Option<String>,
    /// Bind address for Prometheus HTTP scrape server (e.g. `0.0.0.0:9401`).
    #[serde(default)]
    pub listen: Option<SocketAddr>,
    /// Prometheus Pushgateway base URL (e.g. `http://pushgateway.lan:9091`).
    #[serde(default)]
    pub push_url: Option<String>,
    /// Push interval in seconds (defaults to 15).
    #[serde(default)]
    pub push_interval_secs: Option<u64>,
    /// Prometheus Pushgateway job label (defaults to `prod-code`).
    #[serde(default)]
    pub job: Option<String>,
    /// Prometheus Pushgateway instance label (defaults to node advertise address).
    #[serde(default)]
    pub instance: Option<String>,
}

/// Standard configuration search paths in order of preference.
pub fn default_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    paths.push(PathBuf::from("/etc/prod-code/gateway.toml"));

    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(&home).join(".config/prod-code/gateway.toml"));
        paths.push(PathBuf::from(&home).join(".prod-code/gateway.toml"));
    }

    paths.push(PathBuf::from("./gateway.toml"));
    paths
}

/// Loads gateway configuration from explicit path or standard candidate paths.
pub fn load_config_file(explicit_path: Option<&Path>) -> Result<Option<GatewayConfigFile>> {
    if let Some(path) = explicit_path {
        if !path.exists() {
            anyhow::bail!("Configuration file not found: {}", path.display());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read configuration file: {}", path.display()))?;
        let config: GatewayConfigFile = toml::from_str(&content)
            .with_context(|| format!("Failed to parse configuration file: {}", path.display()))?;
        return Ok(Some(config));
    }

    for path in default_config_paths() {
        if path.is_file()
            && let Ok(content) = std::fs::read_to_string(&path)
            && let Ok(config) = toml::from_str::<GatewayConfigFile>(&content)
        {
            tracing::info!(config_file = %path.display(), "Loaded gateway configuration file");
            return Ok(Some(config));
        }
    }

    Ok(None)
}
