/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use super::detect::{Kind, Schema, is_structural, kind_of, text_comment_before};
use super::types::{Occurrence, Variant};

/// Directories that never hold sources worth rewriting.
const SKIP_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".next",
    ".svelte-kit",
    ".build",
    ".prod",
    "Pods",
];

/// Every file worth scanning under `root`.
pub(crate) fn walk(root: &Path, max_bytes: u64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
            } else if meta.is_file()
                && meta.len() <= max_bytes
                && !matches!(kind_of(&path), Kind::Skip)
            {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// Is the character at `col` (1-based, characters) inside a quoted string on this line?
///
/// Counting quotes on one line is not a parser, and it does not have to be: what it decides is
/// whether an occurrence may be edited as text, and a wrong answer shows up in the diff the
/// caller reads before anything is written.
pub(crate) fn inside_quotes(line: &str, col: u32) -> bool {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in line.chars().enumerate() {
        if i as u32 + 1 >= col {
            break;
        }
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') => escaped = true,
            (None, '"') | (None, '\'') | (None, '`') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            _ => {}
        }
    }
    quote.is_some()
}

pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Every whole-word occurrence of any variant in one file.
pub(crate) fn scan(text: &str, variants: &[Variant], file: &Path) -> Vec<Occurrence> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        for (v, variant) in variants.iter().enumerate() {
            let needle: Vec<char> = variant.from.chars().collect();
            if needle.is_empty() || needle.len() > chars.len() {
                continue;
            }
            for start in 0..=(chars.len() - needle.len()) {
                if chars[start..start + needle.len()] != needle[..] {
                    continue;
                }
                let before_ok = start == 0 || !is_word_char(chars[start - 1]);
                let after = start + needle.len();
                let after_ok = after >= chars.len() || !is_word_char(chars[after]);
                if !before_ok || !after_ok {
                    continue;
                }
                let col = start as u32 + 1;
                out.push(Occurrence {
                    file: file.to_path_buf(),
                    line: n as u32 + 1,
                    col,
                    len: needle.len(),
                    variant: v,
                    in_string: inside_quotes(line, col),
                });
            }
        }
    }
    out
}

/// An LSP text edit replacing one occurrence with the variant's new spelling.
pub(crate) fn edit_for(occurrence: &Occurrence, variant: &Variant) -> serde_json::Value {
    serde_json::json!({
        "range": {
            "start": { "line": occurrence.line - 1, "character": occurrence.col - 1 },
            "end": { "line": occurrence.line - 1, "character": occurrence.col - 1 + occurrence.len as u32 }
        },
        "newText": variant.to
    })
}

/// The LSP position for a scanned occurrence. `scan` deliberately counts Unicode scalar
/// values, which makes textual edits straightforward; LSP character offsets are UTF-16 code
/// units, so convert only at the analyzer boundary.
pub(crate) fn lsp_position(text: &str, occurrence: &Occurrence) -> (u32, u32) {
    let character = text
        .lines()
        .nth(occurrence.line.saturating_sub(1) as usize)
        .map(|line| {
            line.chars()
                .take(occurrence.col.saturating_sub(1) as usize)
                .map(|c| c.len_utf16() as u32)
                .sum()
        })
        .unwrap_or_default();
    (occurrence.line.saturating_sub(1), character)
}

/// Whether phase two may replace this occurrence directly.
///
/// Only schemas and queries without an analyzer own bare text. Other text files are evidence:
/// keep their hits in `left` so a README, shell command, configuration value, or plain prose
/// cannot turn a schema rename into a broad find-and-replace.
pub(crate) fn text_rewrite_decision(
    kind: Kind,
    schema: Option<Schema>,
    text: &str,
    occurrence: &Occurrence,
) -> (bool, &'static str) {
    match (kind, schema) {
        (Kind::Text("json"), Some(Schema::Json)) => (
            is_structural(Schema::Json, text, occurrence)
                && !text_comment_before(&occurrence.file, text, occurrence),
            " (not a structured JSON property key)",
        ),
        (Kind::Text("yaml"), Some(Schema::Yaml)) => (
            is_structural(Schema::Yaml, text, occurrence)
                && !text_comment_before(&occurrence.file, text, occurrence),
            " (not a structured YAML key)",
        ),
        (Kind::Text(_), Some(schema)) => (
            is_structural(schema, text, occurrence),
            match schema {
                Schema::OpenApi => " (prose in the OpenAPI document, not the field)",
                Schema::GraphQl => " (a GraphQL comment or description)",
                Schema::Json => " (not a structured JSON property key)",
                Schema::Yaml => " (not a structured YAML key)",
            },
        ),
        (Kind::Text("protobuf" | "sql"), None) => (true, ""),
        (Kind::Code(_), None) => (occurrence.in_string, ""),
        _ => (false, " (unsupported text evidence)"),
    }
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
