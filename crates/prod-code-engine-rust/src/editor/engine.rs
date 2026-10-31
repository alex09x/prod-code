/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use ra_ap_ide::TextRange;
use serde_json::{Value, json};
use std::path::Path;

use super::lines::Lines;
use crate::{RustEngine, line_col_to_offset};

impl RustEngine {
    /// One editor request, answered on the current state of the engine (see
    /// [`RustEngineSnapshot::editor_request`]).
    pub fn editor_request(&self, method: &str, params: &Value) -> Option<Result<Value>> {
        self.snapshot().editor_request(method, params)
    }

    /// [`Self::editor_request`] for an editor that advertised `caps` in `initialize` (#1516).
    pub fn editor_request_with(
        &self,
        method: &str,
        params: &Value,
        caps: &super::caps::EditorCaps,
    ) -> Option<Result<Value>> {
        self.snapshot().editor_request_with(method, params, caps)
    }

    /// The diagnostics of `path` as LSP `Diagnostic`s, for `textDocument/publishDiagnostics`:
    /// what [`RustEngine::diagnostics`] reports, placed in UTF-16 columns.
    pub fn editor_diagnostics(&self, path: &Path) -> Result<Value> {
        let diagnostics = self.diagnostics(path)?;
        let snapshot = self.snapshot();
        let file_id = snapshot
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {}", path.display()))?;
        let text = snapshot.analysis()?.file_text(file_id)?;
        let lines = Lines::new(&text);
        let items: Vec<Value> = diagnostics
            .into_iter()
            .filter(|d| d.severity != "allow")
            .map(|d| {
                let start = line_col_to_offset(&text, d.line, d.col).unwrap_or_default();
                let end = line_col_to_offset(&text, d.end_line, d.end_col).unwrap_or(start);
                let severity = match d.severity.as_str() {
                    "error" => 1,
                    "warning" => 2,
                    "weak" => 4,
                    _ => 3,
                };
                // rust-analyzer uses E0559 for field access, while rustc uses E0609.
                let code = if d.code == "E0559" && d.message.starts_with("no field `") {
                    "E0609"
                } else {
                    &d.code
                };
                let mut out = json!({
                    "range": lines.range(TextRange::new(start, end.max(start))),
                    "severity": severity,
                    "code": code,
                    "source": "prod-code",
                    "message": d.message,
                });
                if let Some(digits) = code.strip_prefix('E')
                    && digits.len() == 4
                    && digits.bytes().all(|byte| byte.is_ascii_digit())
                {
                    out["codeDescription"] = json!({
                        "href": format!("https://doc.rust-lang.org/error_codes/{code}.html")
                    });
                }
                if d.unused {
                    out["tags"] = json!([1]);
                }
                out
            })
            .collect();
        Ok(json!(items))
    }
}
