/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result};

use super::candidates::{check_span, line_bounds};
use super::lsp::{Decl, Pos, Span, parse_decls};
use crate::session::LspSession;

/// Everything the slicer needs to know about one file, fetched once.
pub(crate) struct FileFacts {
    pub(crate) text: String,
    pub(crate) lines: Vec<Range<usize>>,
    pub(crate) decls: Vec<Decl>,
}

impl FileFacts {
    pub(crate) fn check(&self, span: Span) -> Result<(), String> {
        check_span(&self.text, &self.lines, span)
    }
}

/// Reads a file and lists its declarations. A declaration whose range or name the file cannot
/// hold makes the answer malformed, like a declaration without a range: slicing it would cut
/// lines that are not there.
pub(crate) async fn file_facts(session: &mut LspSession, file: &Path) -> Result<FileFacts> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(_) => {
            let (bytes, _) = crate::remote_fs::read_source(
                session.remote(),
                session.root(),
                &file.to_string_lossy(),
            )
            .await
            .with_context(|| format!("cannot read {}", file.display()))?;
            String::from_utf8(bytes).or_else(|e| {
                Ok::<String, anyhow::Error>(String::from_utf8_lossy(&e.into_bytes()).into_owned())
            })?
        }
    };
    let uri = session.uri_for(file)?;
    let symbols = session
        .query(
            file,
            "textDocument/documentSymbol",
            serde_json::json!({ "textDocument": { "uri": uri } }),
        )
        .await?;
    let lines = line_bounds(&text);
    let decls = parse_decls(&symbols)
        .and_then(|decls| {
            for decl in &decls {
                check_span(&text, &lines, decl.range)
                    .and_then(|()| check_span(&text, &lines, decl.selection))
                    .map_err(|e| format!("symbol `{}`: {e}", decl.name))?;
            }
            Ok(decls)
        })
        .map_err(|e| {
            anyhow::anyhow!(
                "malformed textDocument/documentSymbol answer for {}: {e}",
                file.display()
            )
        })?;
    Ok(FileFacts { text, lines, decls })
}

/// The declaration at `pos`: the innermost one whose name is there, else the smallest one
/// spanning its line, preferring one whose range covers the position itself.
pub(crate) fn decl_at(decls: &[Decl], pos: Pos) -> Option<&Decl> {
    decls
        .iter()
        .filter(|d| d.selection.contains(pos))
        .min_by_key(|d| d.range.lines())
        .or_else(|| {
            decls
                .iter()
                .filter(|d| d.range.start.line <= pos.line && pos.line <= d.range.end.line)
                .min_by_key(|d| (d.range.lines(), !d.range.contains(pos)))
        })
}
