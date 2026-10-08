/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

/// Top-level configuration for prod-code MCP.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(default)]
    pub watch: WatchConfig,
    #[serde(default)]
    pub mcp: McpConfig,
}

/// Filesystem watching and change tracking options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WatchConfig {
    /// Additional glob patterns or directory names to ignore.
    #[serde(default)]
    pub ignore: Vec<String>,
    /// Whether to load and apply `.gitignore` / `.ignore` files. Defaults to true.
    #[serde(default = "default_true")]
    pub use_gitignore: bool,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            ignore: Vec::new(),
            use_gitignore: true,
        }
    }
}

/// JSON-RPC transport and frame bounding options.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpConfig {
    /// Maximum frame size in bytes for JSON-RPC messages over stdio.
    /// Defaults to 512 KiB (524,288 bytes).
    #[serde(default = "default_max_frame_bytes")]
    pub max_frame_bytes: usize,
}

pub const DEFAULT_MAX_FRAME_BYTES: usize = 512 * 1024;

fn default_true() -> bool {
    true
}

fn default_max_frame_bytes() -> usize {
    DEFAULT_MAX_FRAME_BYTES
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            max_frame_bytes: default_max_frame_bytes(),
        }
    }
}
