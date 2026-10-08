/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Configuration loading and ignore definitions for prod-code MCP.

pub mod ignore;
pub mod types;

pub use ignore::{IgnoreSnapshot, is_baseline_ignored, is_config_or_ignore_file};
pub use types::{Config, DEFAULT_MAX_FRAME_BYTES, McpConfig, WatchConfig};

use std::path::{Path, PathBuf};

/// Finds the global configuration file paths in precedence order.
pub fn global_config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        let home = Path::new(&home);
        paths.push(home.join(".config").join("prod-code").join("config.toml"));
        paths.push(home.join(".prod-code").join("config.toml"));
    }
    paths
}

/// Finds the workspace configuration file in `root` in precedence order.
pub fn workspace_config_path(root: &Path) -> Option<PathBuf> {
    let dot_prod = root.join(".prod-code.toml");
    if dot_prod.is_file() {
        return Some(dot_prod);
    }
    let prod = root.join("prod-code.toml");
    if prod.is_file() {
        return Some(prod);
    }
    None
}

/// Loads configuration for `root`, merging global configuration first and
/// workspace-specific `.prod-code.toml` / `prod-code.toml` second.
pub fn load_config(root: &Path) -> Config {
    let mut config = Config::default();

    // 1. Global config
    for path in global_config_paths() {
        if path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(parsed) = toml::from_str::<Config>(&content) {
                    config = merge_config(config, parsed);
                    break;
                }
            }
        }
    }

    // 2. Workspace config
    if let Some(ws_path) = workspace_config_path(root) {
        if let Ok(content) = std::fs::read_to_string(&ws_path) {
            if let Ok(parsed) = toml::from_str::<Config>(&content) {
                config = merge_config(config, parsed);
            }
        }
    }

    config
}

/// Merges `incoming` into `base` (incoming takes precedence).
fn merge_config(mut base: Config, incoming: Config) -> Config {
    for pat in incoming.watch.ignore {
        if !base.watch.ignore.contains(&pat) {
            base.watch.ignore.push(pat);
        }
    }
    base.watch.use_gitignore = incoming.watch.use_gitignore;
    if incoming.mcp.max_frame_bytes != DEFAULT_MAX_FRAME_BYTES {
        base.mcp.max_frame_bytes = incoming.mcp.max_frame_bytes;
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_workspace_prod_code_toml() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        let toml_str = r#"
[watch]
ignore = ["build", "docs/gen"]
use_gitignore = true

[mcp]
max_frame_bytes = 1048576
"#;
        std::fs::write(root.join(".prod-code.toml"), toml_str).unwrap();

        let config = load_config(root);
        assert_eq!(config.watch.ignore, vec!["build", "docs/gen"]);
        assert!(config.watch.use_gitignore);
        assert_eq!(config.mcp.max_frame_bytes, 1048576);
    }
}
