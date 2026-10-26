/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, ensure};
use std::path::Path;

/// Reject the complete unsupported scope before scanning, requesting edits or writing.
/// Prune uses the Rust engine's custom safeDelete, not the standalone polyglot tool.
pub(super) fn require_rust_scope(root: &Path) -> Result<()> {
    ensure!(
        crate::sync::expected_engine(root) == Some("rust"),
        "prune is Rust-only; managed-language deletion is unsupported; nothing was written"
    );
    for file in crate::sync::scan_workspace_files(root, None)? {
        let path = Path::new(&file.relative_path);
        if let Some(language) = crate::sync::engine::project::engine_for_file(path) {
            ensure!(
                matches!(
                    language,
                    "rust" | "markdown" | "yaml" | "toml" | "json" | "html" | "css"
                ),
                "prune is Rust-only; mixed-language scope contains {} ({language}); nothing was written",
                file.relative_path
            );
        }
    }
    Ok(())
}
