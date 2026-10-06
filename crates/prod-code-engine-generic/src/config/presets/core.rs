/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::readiness::{INDEX_WAIT, ReadySignal, pyright_found_sources};
use std::collections::HashMap;

use crate::config::discovery::{native_typescript_lsp, npm_global_root, which_bin};
use crate::config::types::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_MAX_RETAINED_DOCUMENTS, DEFAULT_REQUEST_TIMEOUT,
    GenericLspConfig,
};

impl GenericLspConfig {
    /// Create a standard configuration for Python language servers.
    pub fn for_python() -> Self {
        let (cmd, args) = if which_bin("basedpyright-langserver").is_ok() {
            (
                "basedpyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if which_bin("pyright-langserver").is_ok() {
            (
                "pyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if which_bin("ruff").is_ok() {
            ("ruff".to_string(), vec!["server".to_string()])
        } else {
            ("pylsp".to_string(), vec![])
        };
        // basedpyright and pyright report no progress; they log `Found N source files` once
        // their program is set up, and hold a question from then on.
        let ready = if cmd.contains("pyright") {
            ReadySignal::Log(pyright_found_sources)
        } else {
            ReadySignal::Unknown
        };

        let mut env = HashMap::new();
        if let Some(val) = std::env::var_os("PROD_CODE_PYTHON_STUB_CACHE") {
            env.insert(
                "PROD_CODE_PYTHON_STUB_CACHE".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        if let Some(val) = std::env::var_os("MYPYPATH") {
            env.insert("MYPYPATH".to_string(), val.to_string_lossy().into_owned());
        }
        if let Some(val) = std::env::var_os("TYPINGS_PATH") {
            env.insert(
                "TYPINGS_PATH".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }

        let initialization_options = if cmd.contains("pyright") {
            let stub_path = std::env::var("PROD_CODE_PYTHON_STUB_CACHE")
                .ok()
                .unwrap_or_else(|| "typings".to_string());
            Some(serde_json::json!({
                "python": {
                    "analysis": {
                        "stubPath": stub_path
                    }
                }
            }))
        } else {
            None
        };

        Self {
            command: cmd.clone(),
            args,
            env,
            working_dir: None,
            initialization_options,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready,
            index_wait: INDEX_WAIT,
            retain_open_documents: cmd.contains("pyright"),
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// Create a configuration for C/C++ (clangd). A `compile_commands.json` at the workspace
    /// root or under `build/` gives clangd the real flags. Without `--use-dirty-headers` clangd
    /// parses an included header from disk even when its proposed text is open, so a check of a
    /// header edit together with its sources judged the sources against the old header (#292).
    pub fn for_cpp() -> Self {
        Self {
            command: "clangd".to_string(),
            args: vec![
                "--background-index".to_string(),
                "--header-insertion=never".to_string(),
                "--use-dirty-headers".to_string(),
                "--log=error".to_string(),
                "--compile-commands-dir=build".to_string(),
            ],
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            // `backgroundIndexProgress`: begun at once, ended when the index is complete.
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// The clangd that only validation sessions reach: [`Self::for_cpp`] without the
    /// background index, since a validation session asks only for the diagnostics of the texts
    /// it opens. The proposed texts stay out of the main server, where clangd kept a closed
    /// document in its index as last built and went on answering `references` from it (#293).
    pub fn for_cpp_validation() -> Self {
        let mut config = Self::for_cpp();
        config.args.retain(|a| !a.starts_with("--background-index"));
        config.args.push("--background-index=false".to_string());
        // Without a background index there is nothing to wait for.
        config.ready = ReadySignal::HoldsQuestions;
        config
    }

    /// Create a configuration for Swift (sourcekit-lsp). On macOS the toolchain's server is
    /// reached through `xcrun` when it is not on PATH.
    pub fn for_swift() -> Self {
        let (cmd, args) = if which_bin("sourcekit-lsp").is_ok() {
            ("sourcekit-lsp".to_string(), vec![])
        } else if cfg!(target_os = "macos") {
            ("xcrun".to_string(), vec!["sourcekit-lsp".to_string()])
        } else {
            ("sourcekit-lsp".to_string(), vec![])
        };
        let mut env = HashMap::new();
        if let Some(val) = std::env::var_os("SWIFTPM_MODULECACHE_OVERRIDE") {
            env.insert(
                "SWIFTPM_MODULECACHE_OVERRIDE".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        if let Some(val) = std::env::var_os("SWIFT_MODULE_CACHE_PATH") {
            env.insert(
                "SWIFT_MODULE_CACHE_PATH".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        if let Some(val) = std::env::var_os("CLANG_MODULE_CACHE_PATH") {
            env.insert(
                "CLANG_MODULE_CACHE_PATH".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        Self {
            command: cmd,
            args,
            env,
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            // It reports reloading the package as progress.
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// Create a standard configuration for TypeScript / JavaScript language servers.
    pub fn for_typescript() -> Self {
        // TypeScript 7 (native) ships its own LSP: `tsc --lsp --stdio` from the platform
        // package. It needs no tsserver and no Node at all, so it wins when present.
        let native = native_typescript_lsp();
        let (cmd, args) = if let Some(native) = native.as_ref() {
            (
                native.to_string_lossy().into_owned(),
                vec!["--lsp".to_string(), "--stdio".to_string()],
            )
        } else if which_bin("vtsls").is_ok() {
            ("vtsls".to_string(), vec!["--stdio".to_string()])
        } else if which_bin("typescript-language-server").is_ok() {
            (
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            )
        } else {
            (
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            )
        };
        // The native server holds a question until its project is loaded; vtsls reports
        // progress notifications ($/progress); typescript-language-server is unknown.
        let ready = if native.is_some() {
            ReadySignal::HoldsQuestions
        } else if cmd.contains("vtsls") {
            ReadySignal::Progress
        } else {
            ReadySignal::Unknown
        };

        let mut env = HashMap::new();
        if let Some(val) = std::env::var_os("PROD_CODE_TS_TYPES_CACHE") {
            let val_str = val.to_string_lossy().into_owned();
            env.insert("PROD_CODE_TS_TYPES_CACHE".to_string(), val_str.clone());
            if let Some(node_path) = std::env::var_os("NODE_PATH") {
                let mut combined = node_path.to_string_lossy().into_owned();
                combined.push(':');
                combined.push_str(&val_str);
                env.insert("NODE_PATH".to_string(), combined);
            } else {
                env.insert("NODE_PATH".to_string(), val_str);
            }
        }

        let initialization_options = if cmd.contains("vtsls") {
            let tsserver_path = npm_global_root()
                .map(|root| root.join("typescript").join("lib"))
                .filter(|lib| lib.join("tsserver.js").exists())
                .map(|lib| lib.to_string_lossy().into_owned());

            let mut vtsls_opts = serde_json::json!({
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
                    "tsserver": {
                        "maxTsServerMemory": 4096
                    }
                }
            });
            if let Some(p) = tsserver_path {
                if let Some(ts) = vtsls_opts
                    .get_mut("typescript")
                    .and_then(|t| t.get_mut("tsserver"))
                    .and_then(|ts| ts.as_object_mut())
                {
                    ts.insert("path".to_string(), serde_json::Value::String(p));
                }
            }
            Some(vtsls_opts)
        } else {
            npm_global_root()
                .map(|root| root.join("typescript").join("lib"))
                .filter(|lib| lib.join("tsserver.js").exists())
                .map(|lib| {
                    serde_json::json!({
                        "tsserver": { "path": lib.to_string_lossy() },
                        "preferences": { "includeInlayParameterNameHints": "none" }
                    })
                })
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

    /// Create a standard configuration for Java language servers (jdtls).
    pub fn for_java() -> Self {
        let (cmd, args) = if which_bin("jdtls").is_ok() {
            ("jdtls".to_string(), vec![])
        } else if which_bin("java-language-server").is_ok() {
            ("java-language-server".to_string(), vec![])
        } else {
            ("jdtls".to_string(), vec![])
        };
        Self {
            command: cmd,
            args,
            env: HashMap::new(),
            working_dir: None,
            initialization_options: Some(serde_json::json!({
                "settings": {
                    "java": {
                        "autobuild": { "enabled": true }
                    }
                },
                "extendedClientCapabilities": {
                    "progressReportProvider": true,
                    "classFileContentsSupport": true
                }
            })),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// Create a standard configuration for Kotlin language servers.
    pub fn for_kotlin() -> Self {
        Self {
            command: "kotlin-language-server".to_string(),
            args: vec![],
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// Create a standard configuration for C# language servers.
    pub fn for_csharp() -> Self {
        let (cmd, args) = if which_bin("csharp-ls").is_ok() {
            ("csharp-ls".to_string(), vec![])
        } else if which_bin("omnisharp").is_ok() {
            ("omnisharp".to_string(), vec!["-lsp".to_string()])
        } else {
            ("csharp-ls".to_string(), vec![])
        };
        Self {
            command: cmd,
            args,
            env: HashMap::new(),
            working_dir: None,
            initialization_options: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            ready: ReadySignal::Progress,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }
}
