/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! TypeScript / JavaScript server selection with a lazily resolved global npm root.

use prod_code_protocol::readiness::{INDEX_WAIT, ReadySignal};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::core::{has_bin, process_env};
use crate::config::discovery::{native_typescript_lsp_with, npm_global_root, which_bin};
use crate::config::types::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_MAX_RETAINED_DOCUMENTS, DEFAULT_REQUEST_TIMEOUT,
    GenericLspConfig,
};

impl GenericLspConfig {
    /// Create a standard configuration for TypeScript / JavaScript language servers.
    ///
    /// `npm root -g` runs only when `tsgo` is not on `$PATH`, and then once (#1794).
    pub fn for_typescript() -> Self {
        Self::typescript_resolved(
            || which_bin("tsgo").ok(),
            npm_global_root,
            has_bin,
            process_env,
        )
    }

    /// [`Self::for_typescript`] with every lookup injected. `npm_root` is called at most
    /// once: not at all when `tsgo` answers, else to look for the native server under
    /// the global install, whose answer then also serves the tsserver path.
    pub(crate) fn typescript_resolved(
        tsgo: impl FnOnce() -> Option<PathBuf>,
        npm_root: impl FnOnce() -> Option<PathBuf>,
        has: impl Fn(&str) -> bool,
        env_var: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        let mut root = LazyNpmRoot::new(npm_root);
        let native = native_server(tsgo, &mut root);
        Self::typescript_with(native, || root.get(), has, env_var)
    }

    /// The command [`Self::for_typescript`] would run, chosen without building its
    /// initialization options, for callers that only ask whether a server is installed.
    pub fn typescript_command() -> String {
        Self::typescript_command_with(|| which_bin("tsgo").ok(), npm_global_root, has_bin)
    }

    /// [`Self::typescript_command`] with every lookup injected; `npm_root` is called at
    /// most once, and not at all when `tsgo` answers.
    pub(crate) fn typescript_command_with(
        tsgo: impl FnOnce() -> Option<PathBuf>,
        npm_root: impl FnOnce() -> Option<PathBuf>,
        has: impl Fn(&str) -> bool,
    ) -> String {
        let mut root = LazyNpmRoot::new(npm_root);
        let native = native_server(tsgo, &mut root);
        server_command(native.as_deref(), has).0
    }

    /// The configuration for an already chosen native server (or none), with the global
    /// npm root resolved only for a Node-based server's tsserver path.
    pub(crate) fn typescript_with(
        native: Option<PathBuf>,
        npm_root: impl FnOnce() -> Option<PathBuf>,
        has: impl Fn(&str) -> bool,
        env_var: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        let (cmd, args) = server_command(native.as_deref(), has);
        // The native server holds a question until its project is loaded; vtsls reports
        // progress notifications ($/progress); typescript-language-server is unknown.
        let ready = if native.is_some() {
            ReadySignal::HoldsQuestions
        } else if cmd == "vtsls" {
            ReadySignal::Progress
        } else {
            ReadySignal::Unknown
        };

        let mut env = HashMap::new();
        if let Some(val) = env_var("PROD_CODE_TS_TYPES_CACHE") {
            let val_str = val.to_string_lossy().into_owned();
            env.insert("PROD_CODE_TS_TYPES_CACHE".to_string(), val_str.clone());
            if let Some(node_path) = env_var("NODE_PATH") {
                let mut combined = node_path.to_string_lossy().into_owned();
                combined.push(':');
                combined.push_str(&val_str);
                env.insert("NODE_PATH".to_string(), combined);
            } else {
                env.insert("NODE_PATH".to_string(), val_str);
            }
        }

        // The native server needs no tsserver, so it never asks for the npm root.
        let initialization_options = if native.is_some() {
            None
        } else {
            let tsserver_lib = npm_root()
                .map(|root| root.join("typescript").join("lib"))
                .filter(|lib| lib.join("tsserver.js").exists())
                .map(|lib| lib.to_string_lossy().into_owned());
            node_server_options(&cmd, tsserver_lib)
        };

        Self {
            command: cmd,
            args,
            env,
            working_dir: None,
            initialization_options,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }
}

/// TypeScript 7 (native) ships its own LSP: `tsc --lsp --stdio` from the platform
/// package. It needs no tsserver and no Node at all, so it wins when present.
fn server_command(native: Option<&Path>, has: impl Fn(&str) -> bool) -> (String, Vec<String>) {
    match native {
        Some(native) => (
            native.to_string_lossy().into_owned(),
            vec!["--lsp".to_string(), "--stdio".to_string()],
        ),
        None if has("vtsls") => ("vtsls".to_string(), vec!["--stdio".to_string()]),
        None => (
            "typescript-language-server".to_string(),
            vec!["--stdio".to_string()],
        ),
    }
}

/// `tsgo` from `$PATH`, else the platform package under the global `typescript` install.
fn native_server<F: FnOnce() -> Option<PathBuf>>(
    tsgo: impl FnOnce() -> Option<PathBuf>,
    root: &mut LazyNpmRoot<F>,
) -> Option<PathBuf> {
    tsgo().or_else(|| native_typescript_lsp_with(None, root.get().as_deref()))
}

fn node_server_options(cmd: &str, tsserver_lib: Option<String>) -> Option<serde_json::Value> {
    if cmd != "vtsls" {
        return tsserver_lib.map(|lib| {
            serde_json::json!({
                "tsserver": { "path": lib },
                "preferences": { "includeInlayParameterNameHints": "none" }
            })
        });
    }
    let mut tsserver = serde_json::json!({ "maxTsServerMemory": 4096 });
    if let Some(lib) = tsserver_lib {
        tsserver["path"] = serde_json::Value::String(lib);
    }
    Some(serde_json::json!({
        "vtsls": {
            "autoUseWorkspaceTsdk": true,
            "experimental": {
                "completion": {
                    "enableServerSideFuzzyMatch": true
                }
            }
        },
        "typescript": {
            "preferences": {
                "includeInlayParameterNameHints": "none"
            },
            "tsserver": tsserver
        }
    }))
}

/// The global npm root, resolved on first use and remembered for later uses.
struct LazyNpmRoot<F> {
    resolve: Option<F>,
    value: Option<PathBuf>,
}

impl<F: FnOnce() -> Option<PathBuf>> LazyNpmRoot<F> {
    fn new(resolve: F) -> Self {
        Self {
            resolve: Some(resolve),
            value: None,
        }
    }

    fn get(&mut self) -> Option<PathBuf> {
        if let Some(resolve) = self.resolve.take() {
            self.value = resolve();
        }
        self.value.clone()
    }
}

#[cfg(test)]
mod tests;
