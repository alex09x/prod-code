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
use ra_ap_ide::{TextRange, TextSize};
use ra_ap_vfs::FileId;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

use crate::RustEngineSnapshot;

/// Line starts of a text, for conversions between byte offsets and LSP positions.
pub struct Lines<'a> {
    text: &'a str,
    starts: Vec<usize>,
}

impl<'a> Lines<'a> {
    pub fn new(text: &'a str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { text, starts }
    }

    /// The end of a line's source text, before its newline delimiter. CRLF is one delimiter:
    /// its carriage return is not an LSP character position.
    fn line_end(&self, line: usize, start: usize) -> usize {
        let end = self
            .starts
            .get(line + 1)
            .map_or(self.text.len(), |&next| next - 1);
        if end < self.text.len() && end > start && self.text.as_bytes()[end - 1] == b'\r' {
            end - 1
        } else {
            end
        }
    }

    /// The LSP position of a byte offset.
    pub fn position(&self, offset: TextSize) -> Value {
        let offset = usize::from(offset).min(self.text.len());
        let line = self.starts.partition_point(|&start| start <= offset) - 1;
        let start = self.starts[line];
        let offset = offset.min(self.line_end(line, start));
        let character: usize = self
            .text
            .get(start..offset)
            .map(|before| before.chars().map(char::len_utf16).sum())
            .unwrap_or(0);
        json!({ "line": line, "character": character })
    }

    pub fn range(&self, range: TextRange) -> Value {
        json!({ "start": self.position(range.start()), "end": self.position(range.end()) })
    }

    /// The byte offset of an LSP position. A column past the end of its line is the line's
    /// end; a line past the end of the text is the text's end.
    pub fn offset(&self, line: u32, character: u32) -> TextSize {
        let Some(&start) = self.starts.get(line as usize) else {
            return TextSize::of(self.text);
        };
        let end = self.line_end(line as usize, start);
        let mut units = 0u32;
        for (byte, ch) in self.text[start..end].char_indices() {
            if units >= character {
                return TextSize::from((start + byte) as u32);
            }
            units += ch.len_utf16() as u32;
        }
        TextSize::from(end as u32)
    }

    /// The byte offset of the LSP position a JSON `position` object names.
    pub(crate) fn offset_of(&self, position: &Value) -> TextSize {
        let field = |name: &str| position.get(name).and_then(Value::as_u64).unwrap_or(0) as u32;
        self.offset(field("line"), field("character"))
    }

    /// The byte range of a JSON LSP `range` object; a reversed range is put in order.
    pub(crate) fn range_of(&self, range: &Value) -> TextRange {
        let start = self.offset_of(&range["start"]);
        let end = self.offset_of(&range["end"]);
        TextRange::new(start.min(end), start.max(end))
    }
}

/// The server path a `file://` URI names, percent-decoded once.
pub(crate) fn path_of(uri: &str) -> PathBuf {
    prod_code_protocol::path::uri_or_path(uri)
}

/// The `file://` URI of a server path, percent-encoded.
pub(crate) fn uri_of(path: &Path) -> String {
    prod_code_protocol::path::file_uri(path)
}

/// The document a request is about: its path, file id and text.
pub(crate) fn document(
    snapshot: &RustEngineSnapshot,
    params: &Value,
) -> Result<(PathBuf, FileId, String)> {
    let uri = params
        .pointer("/textDocument/uri")
        .and_then(Value::as_str)
        .context("the request names no textDocument.uri")?;
    let path = path_of(uri);
    let file_id = snapshot
        .file_id_for_path(&path)
        .with_context(|| format!("File not found in VFS: {}", path.display()))?;
    let text = snapshot.analysis.file_text(file_id)?.to_string();
    Ok((path, file_id, text))
}
