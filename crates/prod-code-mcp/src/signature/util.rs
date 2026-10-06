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
use std::path::Path;

/// The text of a file the change reads, a caller or one it rewrites. A file that cannot be read
/// stops the change: read as empty, its calls would be neither checked nor rewritten.
pub fn read_caller(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}; nothing was written", path.display()))
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// A parameter list on one line, for the report.
pub fn normalize(list: &str) -> String {
    let one_line = list.split_whitespace().collect::<Vec<_>>().join(" ");
    one_line.trim_end_matches(',').to_string()
}

/// 1-based (line, UTF-16 column) of byte `offset` of `text`, as LSP positions count it. An offset
/// past the end of the text, inside a character, or between the `\r` and the `\n` of a line
/// break is on no position: `None`, not the nearest one.
pub fn line_col_at(text: &str, offset: usize) -> Option<(u32, u32)> {
    if !text.is_char_boundary(offset) {
        return None;
    }
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |n| n + 1);
    let in_line = &before[line_start..];
    if in_line.ends_with('\r') && text[offset..].starts_with('\n') {
        return None;
    }
    let line = u32::try_from(before.matches('\n').count()).ok()?;
    let col = u32::try_from(in_line.encode_utf16().count()).ok()?;
    Some((line.checked_add(1)?, col.checked_add(1)?))
}

/// [`line_col_at`] for a planner, whose offsets come from the text itself: one that is on no
/// position is an error that stops the plan.
pub fn position_at(text: &str, offset: usize) -> Result<(u32, u32)> {
    line_col_at(text, offset).with_context(|| {
        format!(
            "byte {offset} of a {}-byte file is on no line and column; nothing was planned",
            text.len()
        )
    })
}
