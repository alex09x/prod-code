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

use super::types::Span;

pub fn lsp_span(text: &str, range: &serde_json::Value) -> Result<Span> {
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

pub fn offset_at(text: &str, line: u32, col: u32) -> Option<usize> {
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

pub fn line_col_utf16(text: &str, offset: usize) -> Result<(u32, u32)> {
    anyhow::ensure!(
        offset <= text.len() && text.is_char_boundary(offset),
        "a source offset is not a UTF-8 boundary"
    );
    let before = &text[..offset];
    Ok((
        u32::try_from(before.matches('\n').count())?,
        u32::try_from(
            before
                .rsplit('\n')
                .next()
                .unwrap_or_default()
                .encode_utf16()
                .count(),
        )?,
    ))
}
