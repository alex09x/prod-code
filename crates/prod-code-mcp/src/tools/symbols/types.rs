/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::path::Path;

/// One `workspace/symbol` hit, positioned on the symbol's name (1-based).
#[derive(Debug, Clone)]
pub struct SymbolHit {
    pub path: std::path::PathBuf,
    pub name: String,
    pub kind: &'static str,
    pub container: Option<String>,
    pub line: u32,
    pub col: u32,
}

impl SymbolHit {
    pub fn render(&self, root: &Path) -> String {
        let rel = self.path.strip_prefix(root).unwrap_or(&self.path).display();
        let container = self
            .container
            .as_deref()
            .map(|c| format!("{c}::"))
            .unwrap_or_default();
        format!(
            "[{}] {container}{} — {rel}:{}:{}",
            self.kind, self.name, self.line, self.col
        )
    }
}

pub(crate) fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

/// An LSP position is zero-based, but every tool position we retain is one-based.  Keep a
/// distinct error type so a nested-project search can still ignore an unavailable server while
/// refusing evidence that a server did return but encoded incorrectly.
#[derive(Debug)]
pub(crate) struct MalformedLspCoordinate(pub(crate) String);

impl std::fmt::Display for MalformedLspCoordinate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MalformedLspCoordinate {}

pub(crate) fn lsp_coordinate(start: &serde_json::Value, field: &str, context: &str) -> Result<u32> {
    let value = start.get(field).ok_or_else(|| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: missing `{field}` coordinate"
        )))
    })?;
    let value = value.as_u64().ok_or_else(|| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: `{field}` must be a non-negative integer"
        )))
    })?;
    let value = u32::try_from(value).map_err(|_| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: `{field}` exceeds u32"
        )))
    })?;
    value.checked_add(1).ok_or_else(|| {
        anyhow::Error::new(MalformedLspCoordinate(format!(
            "malformed LSP {context}: `{field}` cannot be converted to a one-based coordinate"
        )))
    })
}

pub(crate) fn lsp_position(start: &serde_json::Value, context: &str) -> Result<(u32, u32)> {
    Ok((
        lsp_coordinate(start, "line", context)?,
        lsp_coordinate(start, "character", context)?,
    ))
}

pub(crate) fn is_malformed_lsp_coordinate(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.is::<MalformedLspCoordinate>())
}

/// The Levenshtein distance between two names, counted in characters.
pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}
