/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// The line span (1-based, inclusive) of the smallest declaration containing `line`, and the
/// name the analyzer gives it.
pub fn span_at(symbols: &serde_json::Value, line: u32) -> Option<(String, u32, u32)> {
    fn walk(nodes: &[serde_json::Value], line: u32, best: &mut Option<(String, u32, u32)>) {
        for node in nodes {
            let range = node
                .get("range")
                .or_else(|| node.get("location").and_then(|l| l.get("range")));
            if let Some(range) = range
                && let (Some(s), Some(e)) = (
                    range.pointer("/start/line").and_then(|l| l.as_u64()),
                    range.pointer("/end/line").and_then(|l| l.as_u64()),
                )
            {
                let (s, e) = (s as u32 + 1, e as u32 + 1);
                let name = node
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_string();
                if s <= line && line <= e && best.as_ref().is_none_or(|(_, bs, be)| e - s < be - bs)
                {
                    *best = Some((name, s, e));
                }
            }
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                walk(children, line, best);
            }
        }
    }
    let mut best = None;
    walk(symbols.as_array().map(|a| a.as_slice())?, line, &mut best);
    best
}

/// The first line of the item once its doc comment and attributes are counted as part of it.
///
/// The analyzer's range starts at `fn`/`struct`; a declaration without its `///` and `#[…]`
/// is a different declaration.
pub fn with_doc_comment(text: &str, start: u32) -> u32 {
    let lines: Vec<&str> = text.lines().collect();
    let mut first = start;
    while first > 1 {
        let above = lines
            .get(first as usize - 2)
            .map(|l| l.trim())
            .unwrap_or("");
        if above.starts_with("///") || above.starts_with("#[") || above.starts_with("//!") {
            first -= 1;
        } else {
            break;
        }
    }
    first
}

/// `text` without lines `start..=end`, and those lines on their own.
///
/// Exactly those lines and no others: the caller adjusts every position below the hole by the
/// number of lines removed, and a cut that also tidied the blank lines around it would make
/// that arithmetic a lie. Whatever blank line is left over is the formatter's business.
pub fn cut(text: &str, start: u32, end: u32) -> (String, String) {
    let lines: Vec<&str> = text.lines().collect();
    let (s, e) = (start as usize - 1, end as usize);
    let item = lines[s.min(lines.len())..e.min(lines.len())].join("\n");
    let mut kept: Vec<&str> = Vec::with_capacity(lines.len());
    kept.extend_from_slice(&lines[..s.min(lines.len())]);
    kept.extend_from_slice(&lines[e.min(lines.len())..]);
    let mut out = kept.join("\n");
    out.push('\n');
    (out, item)
}

/// `item` appended to `text`, with exactly one blank line between them.
pub fn append_item(text: &str, item: &str) -> String {
    let mut out = text.trim_end().to_string();
    out.push_str("\n\n");
    out.push_str(item.trim_end());
    out.push('\n');
    out
}

/// The byte offset of a 1-based line and column.
pub(crate) fn offset_of(text: &str, line: u32, col: u32) -> Option<usize> {
    let mut at = 0;
    for (n, l) in text.lines().enumerate() {
        if n as u32 + 1 == line {
            let mut chars = l.char_indices();
            return Some(
                at + chars
                    .nth(col as usize - 1)
                    .map(|(i, _)| i)
                    .unwrap_or(l.len()),
            );
        }
        at += l.len() + 1;
    }
    None
}
