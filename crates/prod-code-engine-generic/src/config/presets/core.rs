/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::readiness::{INDEX_WAIT, ReadySignal, SETTLE, pyright_found_sources};
use std::collections::HashMap;
use std::ffi::OsString;
use std::time::Duration;

use crate::config::discovery::which_bin;
use crate::config::types::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_MAX_RETAINED_DOCUMENTS, DEFAULT_REQUEST_TIMEOUT,
    GenericLspConfig,
};

impl GenericLspConfig {
    /// Create a standard configuration for Python language servers.
    pub fn for_python() -> Self {
        Self::python_with(has_bin, process_env)
    }

    /// [`Self::for_python`] with the `$PATH` lookup and the environment injected.
    pub(crate) fn python_with(
        has: impl Fn(&str) -> bool,
        env_var: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        let (cmd, args) = if has("basedpyright-langserver") {
            (
                "basedpyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if has("pyright-langserver") {
            (
                "pyright-langserver".to_string(),
                vec!["--stdio".to_string()],
            )
        } else if has("ruff") {
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
        if let Some(val) = env_var("PROD_CODE_PYTHON_STUB_CACHE") {
            env.insert(
                "PROD_CODE_PYTHON_STUB_CACHE".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        if let Some(val) = env_var("MYPYPATH") {
            env.insert("MYPYPATH".to_string(), val.to_string_lossy().into_owned());
        }
        if let Some(val) = env_var("TYPINGS_PATH") {
            env.insert(
                "TYPINGS_PATH".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }

        let initialization_options = if cmd.contains("pyright") {
            let stub_path = env_var("PROD_CODE_PYTHON_STUB_CACHE")
                .and_then(|v| v.into_string().ok())
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
            settle: SETTLE,
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
            settle: SETTLE,
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
        Self::swift_with(has_bin, process_env, cfg!(target_os = "macos"))
    }

    /// [`Self::for_swift`] with the `$PATH` lookup, the environment and the host OS injected.
    pub(crate) fn swift_with(
        has: impl Fn(&str) -> bool,
        env_var: impl Fn(&str) -> Option<OsString>,
        macos: bool,
    ) -> Self {
        let (cmd, args) = if has("sourcekit-lsp") {
            ("sourcekit-lsp".to_string(), vec![])
        } else if macos {
            ("xcrun".to_string(), vec!["sourcekit-lsp".to_string()])
        } else {
            ("sourcekit-lsp".to_string(), vec![])
        };
        let mut env = HashMap::new();
        if let Some(val) = env_var("SWIFTPM_MODULECACHE_OVERRIDE") {
            env.insert(
                "SWIFTPM_MODULECACHE_OVERRIDE".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        if let Some(val) = env_var("SWIFT_MODULE_CACHE_PATH") {
            env.insert(
                "SWIFT_MODULE_CACHE_PATH".to_string(),
                val.to_string_lossy().into_owned(),
            );
        }
        if let Some(val) = env_var("CLANG_MODULE_CACHE_PATH") {
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
            // It reports reloading the package as progress; `sourcekit-lsp` 6.2.4 begins its
            // first `Indexing` item ~1.4 s after `initialized`, past the 500 ms default (#1969).
            // Bound initial index wait to 4 s so cold Swift semantic queries return promptly with
            // an indexing note (#1113), while `sourcekit-lsp` 5.10 (no progress) settles out at
            // 3 s as `ReadyState::Unknown` without a bogus starting note.
            ready: ReadySignal::Progress,
            settle: Duration::from_secs(3),
            index_wait: Duration::from_secs(4),
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// Create a standard configuration for Java language servers (jdtls).
    pub fn for_java() -> Self {
        Self::java_with(has_bin)
    }

    /// [`Self::for_java`] with the `$PATH` lookup injected.
    pub(crate) fn java_with(has: impl Fn(&str) -> bool) -> Self {
        let (cmd, args) = if has("jdtls") {
            ("jdtls".to_string(), vec![])
        } else if has("java-language-server") {
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
            settle: SETTLE,
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
            settle: SETTLE,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }

    /// Create a standard configuration for C# language servers.
    pub fn for_csharp() -> Self {
        Self::csharp_with(has_bin)
    }

    /// [`Self::for_csharp`] with the `$PATH` lookup injected.
    pub(crate) fn csharp_with(has: impl Fn(&str) -> bool) -> Self {
        let (cmd, args) = if has("csharp-ls") {
            ("csharp-ls".to_string(), vec![])
        } else if has("omnisharp") {
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
            settle: SETTLE,
            index_wait: INDEX_WAIT,
            retain_open_documents: false,
            max_retained_documents: DEFAULT_MAX_RETAINED_DOCUMENTS,
            health_probe_interval: Some(DEFAULT_HEALTH_PROBE_INTERVAL),
        }
    }
}

pub(super) fn has_bin(name: &str) -> bool {
    which_bin(name).is_ok()
}

pub(super) fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

#[cfg(test)]
mod tests;
