/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

use super::super::exec::{ExecChunk, ExecRequest, ExecUsage};
use super::argv::build_argv;

/// Target language for polyglot remote build and test execution (Roadmap 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteExecLanguage {
    Rust,
    Go,
    Cpp,
    TypeScript,
    Python,
    Swift,
    Generic,
}

impl RemoteExecLanguage {
    pub fn as_str(&self) -> &'static str {
        match self {
            RemoteExecLanguage::Rust => "rust",
            RemoteExecLanguage::Go => "go",
            RemoteExecLanguage::Cpp => "cpp",
            RemoteExecLanguage::TypeScript => "typescript",
            RemoteExecLanguage::Python => "python",
            RemoteExecLanguage::Swift => "swift",
            RemoteExecLanguage::Generic => "generic",
        }
    }
}

impl std::fmt::Display for RemoteExecLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Target verification action or custom command (Roadmap 6.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteExecCommand {
    Check,
    Test,
    Lint,
    Bench,
    #[serde(untagged)]
    Custom(String),
}

impl RemoteExecCommand {
    pub fn as_str(&self) -> &str {
        match self {
            RemoteExecCommand::Check => "check",
            RemoteExecCommand::Test => "test",
            RemoteExecCommand::Lint => "lint",
            RemoteExecCommand::Bench => "bench",
            RemoteExecCommand::Custom(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for RemoteExecCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Output format for remote execution streams: raw streaming or structured json (Roadmap 6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RemoteExecFormat {
    #[default]
    Raw,
    Json,
}

impl RemoteExecFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            RemoteExecFormat::Raw => "raw",
            RemoteExecFormat::Json => "json",
        }
    }
}

/// High-level typed remote execution request across supported languages (Roadmap 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecRequest {
    pub client_workspace_root: String,
    #[serde(default)]
    pub base_workspace_name: Option<String>,
    pub language: RemoteExecLanguage,
    pub command: RemoteExecCommand,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    #[serde(default)]
    pub format: RemoteExecFormat,
    #[serde(default)]
    pub timeout_secs: u64,
    #[serde(default)]
    pub pull_changes: bool,
    #[serde(default)]
    pub subdir: Option<String>,
    #[serde(default)]
    pub client_agent: Option<String>,
    #[serde(default)]
    pub client_host: Option<String>,
}

impl RemoteExecRequest {
    /// Constructs the toolchain command line (argv) for this execution request.
    pub fn to_argv(&self) -> Vec<String> {
        build_argv(self)
    }

    /// Converts this high-level request into the low-level `ExecRequest` executed by the gateway.
    pub fn into_exec_request(self) -> ExecRequest {
        let command = self.to_argv();
        ExecRequest {
            client_workspace_root: self.client_workspace_root,
            base_workspace_name: self.base_workspace_name,
            command,
            env: self.env,
            timeout_secs: self.timeout_secs,
            pull_changes: self.pull_changes,
            subdir: self.subdir,
            client_agent: self.client_agent,
            client_host: self.client_host,
        }
    }
}

/// One source span within a diagnostic event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecSpan {
    pub file: String,
    pub line_start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    pub col_start: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub col_end: Option<u32>,
    #[serde(default)]
    pub is_primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// A structured compiler or linter diagnostic finding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecDiagnostic {
    pub level: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<RemoteExecSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rendered: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,
}

/// A structured test or benchmark execution event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum RemoteExecTestEvent {
    Started {
        name: String,
    },
    Passed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
    },
    Failed {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assertion_diff: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        backtrace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    Skipped {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Bench {
        name: String,
        estimate: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        range: Option<String>,
    },
}

impl RemoteExecTestEvent {
    pub fn name(&self) -> &str {
        match self {
            Self::Started { name }
            | Self::Passed { name, .. }
            | Self::Failed { name, .. }
            | Self::Skipped { name, .. }
            | Self::Bench { name, .. } => name,
        }
    }
}

/// Real-time streaming message emitted during remote execution (Roadmap 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteExecStream {
    Chunk(ExecChunk),
    Diagnostic(RemoteExecDiagnostic),
    TestEvent(RemoteExecTestEvent),
}

/// Final execution verdict and summary across compiler and test runs (Roadmap 6.1).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteExecResult {
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub server_workspace_root: String,
    #[serde(default)]
    pub timed_out: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<ExecUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<RemoteExecDiagnostic>,
    #[serde(default)]
    pub tests_passed: u64,
    #[serde(default)]
    pub tests_failed: u64,
    #[serde(default)]
    pub tests_skipped: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_failures: Vec<RemoteExecTestEvent>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub benches: Vec<RemoteExecTestEvent>,
}

impl RemoteExecResult {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && self.error.is_none()
    }
}
