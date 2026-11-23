/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Rust through Rust Intelligence (`ridx lsp`), the gateway's Rust server, run as an ordinary
//! language server.

use prod_code_protocol::readiness::ReadySignal;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::types::GenericLspConfig;

/// Which server answers Rust.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RustServer {
    /// rust-analyzer: the in-process engine for tools, the `rust-analyzer` binary for editors.
    RustAnalyzer,
    /// Rust Intelligence: `ridx lsp` for tools and editors alike. The gateway's default.
    Ridx,
}

static CONFIGURED: OnceLock<RustServer> = OnceLock::new();

impl RustServer {
    /// `ridx` or `rust-analyzer`, as `--rust-server` / `PROD_CODE_RUST_SERVER` spell it.
    pub fn parse(value: &str) -> Result<Self, String> {
        match value.trim() {
            v if v.eq_ignore_ascii_case("ridx") => Ok(Self::Ridx),
            v if v.eq_ignore_ascii_case("rust-analyzer") => Ok(Self::RustAnalyzer),
            other => Err(format!(
                "unknown Rust server `{other}`: use `ridx` or `rust-analyzer`"
            )),
        }
    }

    /// Records the server's choice, once, at startup. Later calls are ignored.
    pub fn configure(self) {
        let _ = CONFIGURED.set(self);
    }

    /// The server the gateway was started with. A process that never configured one (a test,
    /// an embedding) keeps the in-process rust-analyzer engine, so its outcome does not depend
    /// on whether the node has ridx installed.
    pub fn configured() -> Self {
        CONFIGURED.get().copied().unwrap_or(Self::RustAnalyzer)
    }
}

/// Where `ridx` is: on `PATH`, else under `~/.local/bin` or `~/.cargo/bin`, where
/// `cargo install --root ~/.local` and a plain `cargo install` put it. A gateway started by a
/// service manager has neither directory on its `PATH`.
pub fn ridx_binary() -> Option<PathBuf> {
    if let Ok(found) = crate::config::discovery::which_bin("ridx") {
        return Some(found);
    }
    let home = std::env::var_os("HOME")?;
    [".local/bin/ridx", ".cargo/bin/ridx"]
        .iter()
        .map(|rel| Path::new(&home).join(rel))
        .find(|p| p.is_file())
}

impl GenericLspConfig {
    /// `ridx lsp`. Readiness is its `ridx-index-*` work-done progress: `end` is sent once the
    /// new index is loaded (Rust Intelligence docs/integration.md §2). `ridx-driver` is found
    /// beside `ridx`; the index build runs `cargo` through rustup, so `PATH` gets the binary's
    /// directory and `~/.cargo/bin` ahead of the gateway's own.
    pub fn for_rust_ridx() -> Self {
        let binary = ridx_binary();
        let mut env = std::collections::HashMap::new();
        let mut dirs: Vec<PathBuf> = binary
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(Path::new(&home).join(".cargo/bin"));
        }
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        if let Ok(joined) = std::env::join_paths(dirs) {
            env.insert("PATH".to_string(), joined.to_string_lossy().into_owned());
        }
        Self {
            command: binary
                .map(|b| b.to_string_lossy().into_owned())
                .unwrap_or_else(|| "ridx".to_string()),
            args: vec!["lsp".to_string()],
            env,
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
    fn the_rust_server_is_named_exactly() {
        for value in ["ridx", "RIDX", " ridx "] {
            assert_eq!(RustServer::parse(value), Ok(RustServer::Ridx), "{value:?}");
        }
        assert_eq!(
            RustServer::parse("rust-analyzer"),
            Ok(RustServer::RustAnalyzer)
        );
        for value in ["", "ridx-lsp", "ra"] {
            assert!(RustServer::parse(value).is_err(), "{value:?}");
        }
        // Never configured in this test process: the in-process engine.
        assert_eq!(RustServer::configured(), RustServer::RustAnalyzer);
    }

    #[test]
    fn ridx_runs_its_lsp_subcommand_and_waits_for_its_index() {
        let config = GenericLspConfig::for_rust_ridx();
        assert!(config.is_ridx(), "{}", config.command);
        assert_eq!(config.args, ["lsp"]);
        let path = config.env.get("PATH").expect("a PATH for the index build");
        assert!(path.contains(".cargo/bin"), "{path}");
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
