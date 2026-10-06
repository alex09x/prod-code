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

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletedFunction {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FunctionRange {
    pub(crate) name: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) name_start: usize,
    pub(crate) name_end: usize,
    pub(crate) receiver: Option<Receiver>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Receiver {
    pub(crate) type_name: String,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) fn same_file(left: &Path, right: &Path) -> bool {
    left == right
        || matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
}

pub(crate) fn lsp_span(text: &str, range: &serde_json::Value) -> Result<Span> {
    let number = |end: &str, field: &str| {
        range
            .pointer(&format!("/{end}/{field}"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value < u32::MAX)
    };
    let (Some(start_line), Some(start_col), Some(end_line), Some(end_col)) = (
        number("start", "line"),
        number("start", "character"),
        number("end", "line"),
        number("end", "character"),
    ) else {
        anyhow::bail!("missing or oversized line/character");
    };
    let start = offset_at(text, start_line, start_col)
        .context("the range start is not a valid UTF-16 source position")?;
    let end = offset_at(text, end_line, end_col)
        .context("the range end is not a valid UTF-16 source position")?;
    anyhow::ensure!(start <= end, "the range runs backwards");
    Ok(Span { start, end })
}

pub(crate) fn offset_at(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut start = 0usize;
    for _ in 0..line {
        start += text[start..].find('\n')? + 1;
    }
    let rest = &text[start..];
    let line_end = rest.find('\n').unwrap_or(rest.len());
    let end = line_end - usize::from(line_end < rest.len() && rest[..line_end].ends_with('\r'));
    let mut units = 0u32;
    for (byte, character) in rest[..end].char_indices() {
        if units >= col {
            return (units == col).then_some(start + byte);
        }
        units = units.checked_add(character.len_utf16() as u32)?;
    }
    (units == col).then_some(start + end)
}

pub(crate) fn line_col_utf16(text: &str, offset: usize) -> Result<(u32, u32)> {
    anyhow::ensure!(
        offset <= text.len() && text.is_char_boundary(offset),
        "a source offset is not a UTF-8 boundary"
    );
    let before = &text[..offset];
    let line = u32::try_from(before.matches('\n').count())?;
    let column = u32::try_from(
        before
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .encode_utf16()
            .count(),
    )?;
    Ok((line, column))
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
