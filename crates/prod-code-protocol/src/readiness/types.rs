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
use std::time::{Duration, Instant};

/// How a server tells that it is ready to answer from its index.
#[derive(Clone, Copy, Debug, Default)]
pub enum ReadySignal {
    /// It reports its loading and indexing as work-done progress (gopls, clangd, sourcekit-lsp):
    /// ready when no work it began is still going.
    Progress,
    /// It reports no progress but logs a line when it is set up (basedpyright and pyright:
    /// `Found 2000 source files`); the function recognises that line.
    Log(fn(&str) -> bool),
    /// It holds a question until it can answer it (the native TypeScript server).
    HoldsQuestions,
    /// Nothing is known: its answers are taken as they come.
    #[default]
    Unknown,
}

/// The work a server is still doing when asked: what it calls it, how far it is, and for how
/// long it has been at it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Busy {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub percentage: Option<u64>,
    /// Milliseconds since the work began.
    pub for_ms: u64,
}

impl Busy {
    /// "indexing (1234/5000, 25%) for 3 s", for a note under an answer.
    pub fn describe(&self) -> String {
        let mut detail = Vec::new();
        if let Some(message) = self.message.as_deref().filter(|m| !m.is_empty()) {
            detail.push(message.to_string());
        }
        if let Some(percentage) = self.percentage {
            detail.push(format!("{percentage}%"));
        }
        let detail = if detail.is_empty() {
            String::new()
        } else {
            format!(" ({})", detail.join(", "))
        };
        format!("{}{detail} for {} s", self.title, self.for_ms / 1000)
    }
}

/// The JSON-RPC member an engine adds to an answer given while its server was still busy, and
/// the notification the gateway turns it into for its client.
pub const BUSY_MEMBER: &str = "prodCodeIndexing";
pub const BUSY_NOTIFICATION: &str = "prod-code/indexing";

/// Requests a server answers from its index, which are empty or partial until it has indexed.
pub fn needs_index(method: &str) -> bool {
    matches!(
        method,
        "workspace/symbol"
            | "textDocument/references"
            | "textDocument/implementation"
            | "textDocument/rename"
            | "callHierarchy/incomingCalls"
            | "callHierarchy/outgoingCalls"
            | "typeHierarchy/supertypes"
            | "typeHierarchy/subtypes"
    )
}

/// How long a question answered from the index waits for the server to finish loading and
/// indexing before it is asked anyway, with a note of how far the server got.
pub const INDEX_WAIT: Duration = Duration::from_secs(30);

/// How long after `initialized` a server that reports progress may take to begin it: gopls and
/// clangd began within 0.15 s on a build node.
pub(crate) const SETTLE: Duration = Duration::from_millis(500);

/// How long a server that announces its readiness in its log is waited for at most: one that
/// never logs the line is not waited on forever.
pub(crate) const LOG_LIMIT: Duration = Duration::from_secs(120);

/// How often a wait looks again when nothing was reported: the settle and log windows end
/// without a message.
pub(crate) const RECHECK: Duration = Duration::from_millis(100);

pub(crate) struct Work {
    pub(crate) token: String,
    pub(crate) title: String,
    pub(crate) message: Option<String>,
    pub(crate) percentage: Option<u64>,
    pub(crate) began: Instant,
}

pub(crate) struct State {
    pub(crate) started: Instant,
    pub(crate) active: Vec<Work>,
    pub(crate) progress_seen: bool,
    pub(crate) logged_ready: bool,
}

/// A progress token as text, whether the server sent it as a string or a number.
pub(crate) fn token_text(token: &serde_json::Value) -> String {
    match token {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Whether a basedpyright or pyright log line says it has found its source files, after which
/// it holds a question until it can answer it.
pub fn pyright_found_sources(line: &str) -> bool {
    line == "No source files found."
        || (line.starts_with("Found ") && line.trim_end().ends_with(" source files"))
        || (line.starts_with("Found ") && line.trim_end().ends_with(" source file"))
}
