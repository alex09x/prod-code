/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::readiness::{INDEX_WAIT, ReadySignal};
use std::collections::HashMap;

use crate::config::discovery::which_bin;
use crate::config::types::{
    DEFAULT_HEALTH_PROBE_INTERVAL, DEFAULT_MAX_RETAINED_DOCUMENTS, DEFAULT_REQUEST_TIMEOUT,
    GenericLspConfig,
};

impl GenericLspConfig {
    /// Create a standard configuration for PHP language servers.
    pub fn for_php() -> Self {
        let (cmd, args) = if which_bin("phpactor").is_ok() {
            ("phpactor".to_string(), vec!["language-server".to_string()])
        } else {
            ("intelephense".to_string(), vec!["--stdio".to_string()])
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

    /// Create a standard configuration for Ruby language servers.
    pub fn for_ruby() -> Self {
        let (cmd, args) = if which_bin("ruby-lsp").is_ok() {
            ("ruby-lsp".to_string(), vec![])
        } else {
            ("solargraph".to_string(), vec!["stdio".to_string()])
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

    /// Create a standard configuration for Dart language servers.
    pub fn for_dart() -> Self {
        Self {
            command: "dart".to_string(),
            args: vec!["language-server".to_string()],
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

    /// Create a standard configuration for Zig language servers (zls).
    pub fn for_zig() -> Self {
        Self {
            command: "zls".to_string(),
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

    /// Create a standard configuration for Elixir language servers.
    pub fn for_elixir() -> Self {
        let (cmd, args) = if which_bin("expert").is_ok() {
            ("expert".to_string(), vec![])
        } else if which_bin("lexical").is_ok() {
            ("lexical".to_string(), vec![])
        } else {
            ("elixir-ls".to_string(), vec![])
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

    /// Create a standard configuration for Scala language servers (metals).
    pub fn for_scala() -> Self {
        Self {
            command: "metals".to_string(),
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

    /// Create a standard configuration for Lua language servers.
    pub fn for_lua() -> Self {
        Self {
            command: "lua-language-server".to_string(),
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

    /// Create a standard configuration for Haskell language servers (haskell-language-server).
    pub fn for_haskell() -> Self {
        let (cmd, args) = if which_bin("haskell-language-server-wrapper").is_ok() {
            (
                "haskell-language-server-wrapper".to_string(),
                vec!["--lsp".to_string()],
            )
        } else {
            (
                "haskell-language-server".to_string(),
                vec!["--lsp".to_string()],
            )
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

    /// Create a standard configuration for OCaml language servers (ocamllsp).
    pub fn for_ocaml() -> Self {
        Self {
            command: "ocamllsp".to_string(),
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

    /// Create a standard configuration for Clojure language servers (clojure-lsp).
    pub fn for_clojure() -> Self {
        Self {
            command: "clojure-lsp".to_string(),
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

    /// Create a standard configuration for Julia language servers.
    pub fn for_julia() -> Self {
        let (cmd, args) = if which_bin("julia-lsp").is_ok() {
            ("julia-lsp".to_string(), vec![])
        } else {
            (
                "julia".to_string(),
                vec![
                    "--startup-file=no".to_string(),
                    "--history-file=no".to_string(),
                    "-e".to_string(),
                    "using LanguageServer; runserver()".to_string(),
                ],
            )
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
