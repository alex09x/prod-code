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
    /// Create a standard configuration for SQL language servers (sqls or sql-language-server).
    pub fn for_sql() -> Self {
        let (cmd, args) = if which_bin("sqls").is_ok() {
            ("sqls".to_string(), vec![])
        } else {
            (
                "sql-language-server".to_string(),
                vec![
                    "up".to_string(),
                    "--method".to_string(),
                    "stdio".to_string(),
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

    /// Create a standard configuration for GraphQL language servers (graphql-lsp).
    pub fn for_graphql() -> Self {
        Self {
            command: "graphql-lsp".to_string(),
            args: vec!["server".to_string(), "-m".to_string(), "stream".to_string()],
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

    /// Create a standard configuration for Protocol Buffers language servers (buf or protols).
    pub fn for_protobuf() -> Self {
        let (cmd, args) = if which_bin("buf").is_ok() {
            (
                "buf".to_string(),
                vec!["beta".to_string(), "lsp".to_string()],
            )
        } else {
            ("protols".to_string(), vec![])
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

    /// Create a standard configuration for Crystal language servers (crystalline or scry).
    pub fn for_crystal() -> Self {
        let (cmd, args) = if which_bin("crystalline").is_ok() {
            ("crystalline".to_string(), vec![])
        } else {
            ("scry".to_string(), vec![])
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

    /// Create a standard configuration for Groovy language servers (groovy-language-server).
    pub fn for_groovy() -> Self {
        Self {
            command: "groovy-language-server".to_string(),
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

    /// Create a standard configuration for Ada language servers (ada_language_server or als).
    pub fn for_ada() -> Self {
        let (cmd, args) = if which_bin("ada_language_server").is_ok() {
            ("ada_language_server".to_string(), vec![])
        } else {
            ("als".to_string(), vec![])
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

    /// Create a standard configuration for V language servers (v-analyzer or vls).
    pub fn for_v() -> Self {
        let (cmd, args) = if which_bin("v-analyzer").is_ok() {
            ("v-analyzer".to_string(), vec![])
        } else {
            ("vls".to_string(), vec![])
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

    /// Create a standard configuration for Racket language servers (racket-langserver).
    pub fn for_racket() -> Self {
        let (cmd, args) = if which_bin("racket-langserver").is_ok() {
            ("racket-langserver".to_string(), vec![])
        } else {
            (
                "racket".to_string(),
                vec!["-l".to_string(), "racket-langserver".to_string()],
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

    /// Create a standard configuration for Terraform language servers (terraform-ls or tofu).
    pub fn for_terraform() -> Self {
        let (cmd, args) = if which_bin("terraform-ls").is_ok() {
            ("terraform-ls".to_string(), vec!["serve".to_string()])
        } else {
            ("tofu".to_string(), vec!["lsp".to_string()])
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

    /// Create a standard configuration for Nix language servers (nil or nixd).
    pub fn for_nix() -> Self {
        let (cmd, args) = if which_bin("nil").is_ok() {
            ("nil".to_string(), vec![])
        } else {
            ("nixd".to_string(), vec![])
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
