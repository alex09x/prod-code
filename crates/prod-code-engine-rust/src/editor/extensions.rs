/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The two rust-analyzer extensions used by native refactoring consumers.

use anyhow::Result;
use ra_ap_ide::FilePosition;
use serde_json::{Value, json};

use super::lines::{Lines, document};
use crate::RustEngineSnapshot;

/// Distinguishes malformed extension requests from analyzer failures at the RPC boundary.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct InvalidExtensionParams(pub String);

impl RustEngineSnapshot {
    /// Upstream's JSON string, including node/token kinds and byte/line/byte-column triples.
    /// The analyzer chooses the file's crate edition and reads this snapshot's exact text.
    pub fn view_syntax_tree(&self, params: &Value) -> Result<Value> {
        let (_, file_id, _) = document(self, params).map_err(invalid_document)?;
        Ok(Value::String(self.analysis()?.view_syntax_tree(file_id)?))
    }

    /// Expand the macro resolved at this occurrence, including scoped definitions and nested
    /// expansion, rather than looking for another macro with the same spelling.
    pub fn expand_macro(&self, params: &Value) -> Result<Value> {
        let (_, file_id, text) = document(self, params).map_err(invalid_document)?;
        let coordinate = |name| {
            params["position"][name]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .ok_or_else(|| {
                    InvalidExtensionParams(format!(
                        "position.{name} must be a non-negative 32-bit integer"
                    ))
                })
        };
        let line = coordinate("line")?;
        let character = coordinate("character")?;
        let lines = Lines::new(&text);
        let offset = lines.offset(line, character);
        // Lines::offset deliberately clamps ordinary editor queries. An extension used for
        // refactoring must reject out-of-file coordinates and split UTF-16 surrogate pairs.
        if lines.position(offset) != json!({"line":line, "character":character}) {
            return Err(InvalidExtensionParams(
                "position is outside the document or splits a UTF-16 character".into(),
            )
            .into());
        }
        let expanded = self
            .analysis()?
            .expand_macro(FilePosition { file_id, offset })?;
        Ok(expanded.map_or(Value::Null, |expanded| {
            json!({
                "name": expanded.name,
                "expansion": expanded.expansion,
            })
        }))
    }
}

fn invalid_document(error: anyhow::Error) -> anyhow::Error {
    if crate::is_salsa_cancelled(&error) {
        error
    } else {
        InvalidExtensionParams(error.to_string()).into()
    }
}
