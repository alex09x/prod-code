/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Rust through Rust Intelligence (`ridx lsp`) as an ordinary language server, the
//! alternative to the in-process rust-analyzer engine.

use prod_code_protocol::readiness::ReadySignal;

use crate::config::types::GenericLspConfig;

/// Which server answers Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustServer {
    /// rust-analyzer: the in-process engine for tools, the `rust-analyzer` binary for editors.
    RustAnalyzer,
    /// Rust Intelligence: `ridx lsp` for tools and editors alike.
    Ridx,
}

impl RustServer {
    /// `PROD_CODE_RUST_SERVER=ridx` selects Rust Intelligence. Unset, or any other value,
    /// selects rust-analyzer.
    pub fn from_env() -> Self {
        Self::parse(std::env::var("PROD_CODE_RUST_SERVER").ok().as_deref())
    }

    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some(v) if v.eq_ignore_ascii_case("ridx") => Self::Ridx,
            _ => Self::RustAnalyzer,
        }
    }
}

impl GenericLspConfig {
    /// `ridx lsp`. Readiness is its `ridx-index-*` work-done progress: `end` is sent once the
    /// new index is loaded (Rust Intelligence docs/integration.md §2). `ridx-driver` must be on
    /// `PATH` or beside `ridx`, with the toolchain it was built for.
    pub fn for_rust_ridx() -> Self {
        Self {
            command: "ridx".to_string(),
            args: vec!["lsp".to_string()],
            ready: ReadySignal::Progress,
            ..Self::default()
        }
    }

    /// Whether this configuration starts Rust Intelligence (`ridx`, by name or path). The
    /// identity is carried in the configuration, so every clone of it (the validation server)
    /// is treated as ridx whatever the server declares in its handshake.
    pub fn is_ridx(&self) -> bool {
        std::path::Path::new(&self.command)
            .file_name()
            .is_some_and(|name| name == "ridx")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ridx_selects_rust_intelligence() {
        for value in ["ridx", "RIDX", " ridx "] {
            assert_eq!(
                RustServer::parse(Some(value)),
                RustServer::Ridx,
                "{value:?}"
            );
        }
        for value in [None, Some(""), Some("rust-analyzer"), Some("ridx-lsp")] {
            assert_eq!(
                RustServer::parse(value),
                RustServer::RustAnalyzer,
                "{value:?}"
            );
        }
    }

    #[test]
    fn ridx_runs_its_lsp_subcommand_and_waits_for_its_index() {
        let config = GenericLspConfig::for_rust_ridx();
        assert_eq!(config.command, "ridx");
        assert_eq!(config.args, ["lsp"]);
        assert!(matches!(config.ready, ReadySignal::Progress));
        assert!(config.is_ridx());
        let mut by_path = config.clone();
        by_path.command = "/opt/ridx-0.3.8/bin/ridx".to_string();
        assert!(by_path.is_ridx());
        for other in ["rust-analyzer", "ridx-driver", "/usr/bin/clangd"] {
            by_path.command = other.to_string();
            assert!(!by_path.is_ridx(), "{other}");
        }
    }
}
