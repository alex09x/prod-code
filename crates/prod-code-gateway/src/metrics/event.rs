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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct Event {
    pub ts_ms: u64,
    /// `lsp`, `exec` or `sync`.
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
        }
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// An event as a daily file holds it.
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
