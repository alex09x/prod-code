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
    /// Create a standard configuration for Markdown language servers (marksman).
    pub fn for_markdown() -> Self {
        Self {
            command: "marksman".to_string(),
            args: vec!["server".to_string()],
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

    /// Create a standard configuration for YAML language servers (yaml-language-server).
    pub fn for_yaml() -> Self {
        Self {
            command: "yaml-language-server".to_string(),
            args: vec!["--stdio".to_string()],
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

    /// Create a standard configuration for TOML language servers (taplo).
    pub fn for_toml() -> Self {
        Self {
            command: "taplo".to_string(),
            args: vec!["lsp".to_string(), "stdio".to_string()],
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

    /// Create a standard configuration for JSON language servers (vscode-json-language-server).
    pub fn for_json() -> Self {
        Self {
            command: "vscode-json-language-server".to_string(),
            args: vec!["--stdio".to_string()],
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

    /// Create a standard configuration for HTML language servers (vscode-html-language-server).
    pub fn for_html() -> Self {
        Self {
            command: "vscode-html-language-server".to_string(),
            args: vec!["--stdio".to_string()],
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

    /// Create a standard configuration for CSS language servers (vscode-css-language-server).
    pub fn for_css() -> Self {
        Self {
            command: "vscode-css-language-server".to_string(),
            args: vec!["--stdio".to_string()],
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

    /// Create a standard configuration for Dockerfile language servers (docker-langserver).
    pub fn for_dockerfile() -> Self {
        let (cmd, args) = if which_bin("docker-langserver").is_ok() {
            ("docker-langserver".to_string(), vec!["--stdio".to_string()])
        } else {
            (
                "dockerfile-language-server-nodejs".to_string(),
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

    /// Create a standard configuration for Svelte language servers (svelteserver).
    pub fn for_svelte() -> Self {
        Self {
            command: "svelteserver".to_string(),
            args: vec!["--stdio".to_string()],
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

    /// Create a standard configuration for Vue language servers (vue-language-server or vls).
    pub fn for_vue() -> Self {
        let (cmd, args) = if which_bin("vue-language-server").is_ok() {
            (
                "vue-language-server".to_string(),
                vec!["--stdio".to_string()],
            )
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

    /// Create a standard configuration for Assembly language servers (asm-lsp).
    pub fn for_assembly() -> Self {
        Self {
            command: "asm-lsp".to_string(),
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
