/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Metrics event structures and timestamps.

use prod_code_protocol::OperationMetric;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub ts_ms: u64,
    /// `lsp`, `exec`, `sync`, `search`, `shadow`, `read_file`, `place`, or `status`.
    pub kind: &'static str,
    pub session_id: u64,
    pub client_name: String,
    pub agent: String,
    pub host: String,
    pub client_addr: String,
    pub workspace: String,
    pub engine: String,
    pub method: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    pub duration_ms: u64,
    pub ok: bool,
    pub items: u64,
    pub command: String,
    pub exit_code: Option<i32>,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
}

impl Event {
    pub fn blank(kind: &'static str) -> Self {
        Self {
            ts_ms: now_ms(),
            kind,
            session_id: 0,
            client_name: String::new(),
            agent: "unknown".to_string(),
            host: "unknown".to_string(),
            client_addr: String::new(),
            workspace: String::new(),
            engine: String::new(),
            method: String::new(),
            file: String::new(),
            line: 0,
            col: 0,
            duration_ms: 0,
            ok: true,
            items: 0,
            command: String::new(),
            exit_code: None,
            bytes: 0,
            error_class: None,
        }
    }

    /// Converts this event into a normalized [`OperationMetric`] with low-cardinality dimensions.
    pub fn to_operation_metric(&self, node: &str) -> OperationMetric {
        let method = if !self.method.is_empty() {
            self.method.clone()
        } else if !self.command.is_empty() {
            command_method(&self.command)
        } else {
            self.kind.to_string()
        };

        let engine = if !self.engine.is_empty() {
            self.engine.clone()
        } else {
            "none".to_string()
        };

        let error_class = if !self.ok {
            self.error_class
                .clone()
                .or_else(|| Some(classify_error(&self.command, self.exit_code).to_string()))
        } else {
            None
        };

        let agent = if !self.agent.is_empty() {
            Some(self.agent.clone())
        } else {
            None
        };

        let host = if !self.host.is_empty() {
            Some(self.host.clone())
        } else {
            None
        };

        let workspace = if !self.workspace.is_empty() {
            if self.workspace.starts_with('/')
                || self.workspace.contains(":\\")
                || self.workspace.contains(":/")
            {
                std::path::Path::new(&self.workspace)
                    .file_name()
                    .and_then(|f| f.to_str())
                    .map(|s| s.to_string())
            } else {
                Some(self.workspace.clone())
            }
        } else {
            None
        };

        OperationMetric {
            ts_ms: self.ts_ms,
            node: node.to_string(),
            category: self.kind.to_string(),
            method,
            engine,
            duration_ms: self.duration_ms,
            ok: self.ok,
            error_class,
            items: self.items,
            bytes: self.bytes,
            exit_code: self.exit_code,
            agent,
            host,
            workspace,
        }
    }
}

/// Normalizes raw command text into a bounded, low-cardinality method category.
pub fn command_method(cmd: &str) -> String {
    let first = cmd.split_whitespace().next().unwrap_or("exec");
    let base = std::path::Path::new(first)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| first.to_string());

    if base == "cargo" {
        if let Some(sub) = cmd.split_whitespace().nth(1) {
            return format!("cargo-{sub}");
        }
    } else if base == "go" {
        if let Some(sub) = cmd.split_whitespace().nth(1) {
            return format!("go-{sub}");
        }
    } else if base == "npm" || base == "pnpm" || base == "yarn" {
        if let Some(sub) = cmd.split_whitespace().nth(1) {
            return format!("{base}-{sub}");
        }
    }
    base
}

/// Categorizes failures into normalized low-cardinality error classes.
pub fn classify_error(detail: &str, exit_code: Option<i32>) -> &'static str {
    if let Some(code) = exit_code {
        if code == 124 || code == -9 {
            return "timeout";
        }
        if code == 130 || code == 137 {
            return "cancelled";
        }
        if code != 0 {
            return "exit_non_zero";
        }
    }

    let lower = detail.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") || lower.contains("deadline") {
        "timeout"
    } else if lower.contains("cancel") || lower.contains("interrupted") {
        "cancelled"
    } else if lower.contains("invalid") || lower.contains("param") {
        "invalid_params"
    } else if lower.contains("not found") || lower.contains("no such") {
        "not_found"
    } else if lower.contains("connection") || lower.contains("io") || lower.contains("broken pipe")
    {
        "io_error"
    } else if lower.contains("admission") || lower.contains("pressure") || lower.contains("reject")
    {
        "admission_rejected"
    } else {
        "internal_error"
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// An event as legacy daily files or stored lines hold it.
#[derive(Deserialize)]
#[serde(default)]
pub(crate) struct Stored {
    pub(crate) ts_ms: u64,
    pub(crate) kind: String,
    pub(crate) agent: String,
    pub(crate) host: String,
    pub(crate) workspace: String,
    pub(crate) method: String,
    pub(crate) duration_ms: u64,
    pub(crate) ok: bool,
    pub(crate) items: u64,
    pub(crate) command: String,
    pub(crate) bytes: u64,
}

impl Default for Stored {
    fn default() -> Self {
        Self {
            ts_ms: 0,
            kind: String::new(),
            agent: String::new(),
            host: String::new(),
            workspace: String::new(),
            method: String::new(),
            duration_ms: 0,
            ok: true,
            items: 0,
            command: String::new(),
            bytes: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_method_normalizes_toolchains() {
        assert_eq!(command_method("cargo check --all"), "cargo-check");
        assert_eq!(command_method("cargo test -p foo"), "cargo-test");
        assert_eq!(command_method("go test ./..."), "go-test");
        assert_eq!(command_method("/usr/bin/pytest tests/"), "pytest");
    }

    #[test]
    fn classify_error_maps_correctly() {
        assert_eq!(classify_error("", Some(124)), "timeout");
        assert_eq!(classify_error("", Some(1)), "exit_non_zero");
        assert_eq!(classify_error("connection reset by peer", None), "io_error");
        assert_eq!(
            classify_error("resource pressure", None),
            "admission_rejected"
        );
    }
}
