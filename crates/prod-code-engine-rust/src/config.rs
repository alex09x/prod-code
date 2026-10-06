/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Per-repository analysis configuration and cargo settings.

use ra_ap_cfg::{CfgAtom, CfgDiff};
use ra_ap_intern::sym;
use ra_ap_project_model::{CargoConfig, CargoFeatures, CfgOverrides, RustLibSource};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Per-repository analysis settings, read from `prod-code.toml` at the workspace root:
///
/// ```toml
/// [rust]
/// features = "all"            # or ["feat-a", "feat-b"]
/// no_default_features = false
/// all_targets = true          # tests, benches and examples are analysed too
/// sysroot = true              # load the standard library sources (rust-src)
/// ```
///
/// Repositories that compile the same source files into several crates behind different
/// feature flags (a `#[path]`-shared module tree) need `features = "all"`: rust-analyzer
/// attaches each file to one crate, and a module behind a disabled feature is dead there.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProdCodeConfig {
    pub rust: RustAnalysisOptions,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct RustAnalysisOptions {
    pub features: FeatureSelection,
    pub no_default_features: bool,
    pub all_targets: bool,
    pub sysroot: bool,
    /// Run build scripts (`cargo check` on load, warm on the gateway) so `OUT_DIR` code and
    /// proc-macro crates exist, and expand proc macros through rust-analyzer's out-of-process
    /// proc-macro server. Without it derives and attribute macros resolve to nothing.
    pub build_scripts: bool,
    /// Execution mode for procedural macros: "sandboxed" (default), "sysroot", or "disabled".
    pub proc_macro_srv: ProcMacroServerKind,
    /// Optional limit on proc-macro worker processes allocated to this workspace.
    /// When None, concurrency is dynamically governed by the shared worker farm.
    pub proc_macro_workers: Option<usize>,
    /// Optional virtual memory / address space limit in megabytes per worker process (default 2048 MB = 2 GiB).
    pub proc_macro_memory_limit_mb: Option<u64>,
}

/// Execution strategy for Rust procedural macro expansion.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ProcMacroServerKind {
    /// Isolated, sandboxed worker pool with memory limits, core dump suppression,
    /// lowered scheduling priority, and secret-scrubbed environment (default).
    #[default]
    Sandboxed,
    /// Direct sysroot proc-macro server without sandboxing wrapper.
    Sysroot,
    /// Completely disabled proc-macro expansion.
    Disabled,
}

impl Default for RustAnalysisOptions {
    fn default() -> Self {
        Self {
            features: FeatureSelection::Selected(Vec::new()),
            no_default_features: false,
            all_targets: true,
            sysroot: true,
            build_scripts: true,
            proc_macro_srv: ProcMacroServerKind::Sandboxed,
            proc_macro_workers: None,
            proc_macro_memory_limit_mb: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum FeatureSelection {
    /// `features = "all"`.
    Keyword(String),
    /// `features = ["a", "b"]`.
    Selected(Vec<String>),
}

impl ProdCodeConfig {
    /// The configuration file name looked up at the workspace root.
    pub const FILE_NAME: &'static str = "prod-code.toml";

    /// Reads `<root>/prod-code.toml`; a missing file is the default configuration and a
    /// malformed one is reported and ignored.
    pub fn load(root: &Path) -> Self {
        let path = root.join(Self::FILE_NAME);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        match toml::from_str::<Self>(&text) {
            Ok(config) => config,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "prod-code.toml ignored");
                Self::default()
            }
        }
    }

    /// The rust-analyzer cargo configuration these options describe.
    pub fn cargo_config(&self) -> CargoConfig {
        let rust = &self.rust;
        let features = match &rust.features {
            FeatureSelection::Keyword(word) if word.eq_ignore_ascii_case("all") => {
                CargoFeatures::All
            }
            FeatureSelection::Keyword(word) => CargoFeatures::Selected {
                features: vec![word.clone()],
                no_default_features: rust.no_default_features,
            },
            FeatureSelection::Selected(features) => CargoFeatures::Selected {
                features: features.clone(),
                no_default_features: rust.no_default_features,
            },
        };
        CargoConfig {
            all_targets: rust.all_targets,
            features,
            sysroot: rust.sysroot.then_some(RustLibSource::Discover),
            // Like rust-analyzer's `cargo.cfgs` default: `cfg(test)` modules and
            // `debug_assertions` code are analysed, so #[test] functions exist in the
            // call graph and the impact analysis sees them.
            cfg_overrides: CfgOverrides {
                global: CfgDiff::new(
                    vec![
                        CfgAtom::Flag(sym::test),
                        CfgAtom::Flag(sym::debug_assertions),
                    ],
                    Vec::new(),
                ),
                ..CfgOverrides::default()
            },
            ..CargoConfig::default()
        }
    }
}
