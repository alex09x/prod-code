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
    /// Create a standard configuration for Shell language servers (bash-language-server).
    pub fn for_shell() -> Self {
        Self {
            command: "bash-language-server".to_string(),
            args: vec!["start".to_string()],
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

    /// Create a standard configuration for R language servers (languageserver).
    pub fn for_r() -> Self {
        let (cmd, args) = if which_bin("r-languageserver").is_ok() {
            ("r-languageserver".to_string(), vec![])
        } else {
            (
                "R".to_string(),
                vec![
                    "--slave".to_string(),
                    "-e".to_string(),
                    "languageserver::run()".to_string(),
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

    /// Create a standard configuration for Erlang language servers (erlang_ls).
    pub fn for_erlang() -> Self {
        Self {
            command: "erlang_ls".to_string(),
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

    /// Create a standard configuration for F# language servers (fsautocomplete).
    pub fn for_fsharp() -> Self {
        Self {
            command: "fsautocomplete".to_string(),
            args: vec!["--adaptive-lsp-server-enabled".to_string()],
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

    /// Create a standard configuration for Perl language servers (pls).
    pub fn for_perl() -> Self {
        let (cmd, args) = if which_bin("pls").is_ok() {
            ("pls".to_string(), vec![])
        } else {
            ("perl-language-server".to_string(), vec![])
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

    /// Create a standard configuration for Solidity language servers.
    pub fn for_solidity() -> Self {
        let (cmd, args) = if which_bin("nomicfoundation-solidity-language-server").is_ok() {
            (
                "nomicfoundation-solidity-language-server".to_string(),
                vec!["--stdio".to_string()],
            )
        } else {
            (
                "solidity-language-server".to_string(),
                vec!["--stdio".to_string()],
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

    /// Create a standard configuration for Nim language servers (nimlsp).
    pub fn for_nim() -> Self {
        Self {
            command: "nimlsp".to_string(),
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

    /// Create a standard configuration for D language servers (serve-d).
    pub fn for_d() -> Self {
        Self {
            command: "serve-d".to_string(),
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

    /// Create a standard configuration for Fortran language servers (fortls).
    pub fn for_fortran() -> Self {
        Self {
            command: "fortls".to_string(),
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
}
