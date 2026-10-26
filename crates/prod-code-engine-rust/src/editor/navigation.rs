/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Type navigation in the exact snapshot selected for an editor session.

use anyhow::Result;
use ra_ap_ide::FilePosition;
use serde_json::{Value, json};

use super::InvalidExtensionParams;
use super::lines::{Lines, document, uri_of};
use crate::RustEngineSnapshot;

impl RustEngineSnapshot {
    /// Resolve nominal types with the pinned analyzer, retaining every candidate. A
    /// consumer must distinguish an empty or ambiguous result from one exact identity.
    pub fn type_definition(&self, params: &Value) -> Result<Value> {
        let uri = params.pointer("/textDocument/uri").and_then(Value::as_str);
        if uri
            .and_then(prod_code_protocol::path::file_uri_path)
            .is_none()
        {
            return Err(
                InvalidExtensionParams("textDocument.uri must be a file URI".into()).into(),
            );
        }
        let (path, file_id, text) = document(self, params).map_err(|error| {
            if crate::is_salsa_cancelled(&error) {
                error
            } else {
                InvalidExtensionParams(error.to_string()).into()
            }
        })?;
        let coordinate = |name| {
            params["position"][name]
                .as_u64()
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value < u32::MAX)
                .ok_or_else(|| InvalidExtensionParams(format!("invalid position.{name}")))
        };
        let (line, character) = (coordinate("line")?, coordinate("character")?);
        let lines = Lines::new(&text);
        let offset = lines.offset(line, character);
        if lines.position(offset) != json!({"line":line, "character":character}) {
            return Err(InvalidExtensionParams(
                "position is outside the document or splits a UTF-16 character".into(),
            )
            .into());
        }
        let overlay = self
            .overlay_for_path(&path)
            .or_else(|| self.overlay_for_path(&self.workspace_root));
        let targets = self
            .analysis()?
            .goto_type_definition(FilePosition { file_id, offset })?
            .map(|info| info.info)
            .unwrap_or_default();
        let mut locations = Vec::new();
        for target in targets {
            if !self.is_in_view(target.file_id, overlay.as_ref()) {
                continue;
            }
            if let Some(path) = self.path_for_file_id_in_view(target.file_id, overlay.as_ref()) {
                let text = self.analysis()?.file_text(target.file_id)?;
                let focus = target.focus_range.unwrap_or(target.full_range);
                locations.push(json!({
                    "uri": uri_of(&path),
                    "range": Lines::new(&text).range(focus),
                }));
            }
        }
        Ok(json!(locations))
    }
}
