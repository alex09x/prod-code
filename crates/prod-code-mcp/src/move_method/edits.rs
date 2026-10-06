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
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// The span of the item that starts at `line` (1-based) together with its doc comment and
/// attributes, as byte offsets of its first line and of the end of its closing line.
pub(crate) fn item_span(text: &str, name_at: usize, body_close: usize) -> (usize, usize) {
    let start = crate::signature::line_col_at(text, name_at)
        .map(|(line, _)| crate::move_item::with_doc_comment(text, line))
        .and_then(|first_line| crate::signature::offset_of(text, first_line, 1))
        .unwrap_or(name_at);
    let end = text[body_close..]
        .find('\n')
        .map_or(text.len(), |i| body_close + i + 1);
    (start, end)
}

/// What to cut to take the item at `span_start..span_end` out of the `impl` whose header
/// starts at `impl_at` and whose braces are at `impl_open` and `impl_close`: the item alone, or
/// the whole block (and the blank line above it) when nothing else is left in it.
pub fn cut_from_impl(
    text: &str,
    (impl_at, impl_open, impl_close): (usize, usize, usize),
    (span_start, span_end): (usize, usize),
) -> (usize, usize) {
    let emptied = text[impl_open + 1..span_start].trim().is_empty()
        && text[span_end..impl_close].trim().is_empty();
    if !emptied {
        return (span_start, span_end);
    }
    let line_start = text[..impl_at].rfind('\n').map_or(0, |i| i + 1);
    let start = if text[..line_start].ends_with("\n\n") {
        line_start - 1
    } else {
        line_start
    };
    let end = text[impl_close..]
        .find('\n')
        .map_or(text.len(), |i| impl_close + i + 1);
    (start, end)
}

/// Where `method_text` goes in `target_text` and what is inserted there: before the closing
/// brace of `target_name`'s inherent `impl`, after a blank line; or, when the type has none, in
/// a new `impl` right after the type's declaration, which starts on `def_line`.
pub(crate) fn insertion(
    target_text: &str,
    target_name: &str,
    def_line: u32,
    method_text: &str,
) -> Result<(usize, String)> {
    let target_impl = crate::extract_field::impl_blocks(target_text)
        .into_iter()
        .find(|(ty, at, o, _)| ty == target_name && !target_text[*at..*o].contains(" for "));
    if let Some((_, _, _, impl_close)) = target_impl {
        let before = target_text[..impl_close].trim_end_matches([' ', '\t']);
        let lead = if before.ends_with("{\n") || before.ends_with('{') {
            ""
        } else {
            "\n"
        };
        return Ok((before.len(), format!("{lead}{method_text}")));
    }
    // After the type's declaration: its closing `}` or `;`.
    let decl_at = crate::signature::offset_of(target_text, def_line, 1)
        .context("the type's declaration is not in its file")?;
    let end = target_text[decl_at..]
        .find(['{', ';'])
        .map(|i| decl_at + i)
        .context("the type's declaration does not end")?;
    let end = if target_text.as_bytes()[end] == b'{' {
        crate::parameter_object::matching_bracket(target_text, end)
            .context("the type's declaration does not close")?
    } else {
        end
    };
    let insert_at = target_text[end..]
        .find('\n')
        .map_or(target_text.len(), |i| end + i + 1);
    Ok((
        insert_at,
        format!("\nimpl {target_name} {{\n{method_text}}}\n"),
    ))
}

/// The errors the analyzer finds in `edits`, one line each.
pub(crate) async fn errors_of(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
) -> Result<Vec<String>> {
    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
    Ok(reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect())
}
