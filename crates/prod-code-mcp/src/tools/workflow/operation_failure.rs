/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde_json::{Value, json};

pub(super) fn before(tool: &str, error: anyhow::Error) -> anyhow::Error {
    let effects = match tool {
        "apply" => json!({"operation":"apply","status":"refused","writes":0}),
        "verify" => json!({"operation":"verify","status":"refused","started":false}),
        _ => return error,
    };
    OperationFailure { error, effects }.into()
}

/// Carries effects separately from human diagnostics so selected outputs cannot hide writes.
#[derive(Debug)]
pub(super) struct OperationFailure {
    pub error: anyhow::Error,
    pub effects: Value,
}

impl std::fmt::Display for OperationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:#}", self.error)
    }
}

impl std::error::Error for OperationFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}
