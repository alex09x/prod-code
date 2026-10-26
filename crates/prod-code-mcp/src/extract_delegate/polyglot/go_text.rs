/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::extract_delegate::common::{is_ident, split_top};

// Retained for callers of the standalone textual preview API. Public extraction uses
// go_literals::literal_edits with analyzer-confirmed key positions instead.
pub fn rewrite_go_literals(
    text: &str,
    owner: &str,
    fields: &[String],
    field: &str,
    helper: &str,
) -> String {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (i, _) in text.match_indices(owner) {
        let before = &text[..i];
        if before.chars().next_back().is_some_and(is_ident)
            || text[i + owner.len()..].chars().next().is_some_and(is_ident)
            || before.trim_end().ends_with("type")
        {
            continue;
        }
        let rest = &text[i + owner.len()..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('{') {
            continue;
        }
        let open = i + owner.len() + (rest.len() - trimmed.len());
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let body = &text[open + 1..close];
        let entries: Vec<&str> = split_top(body, ',')
            .into_iter()
            .map(|(s, e)| body[s..e].trim())
            .filter(|e| !e.is_empty())
            .collect();
        let key = |e: &str| -> String { e.split(':').next().unwrap_or(e).trim().to_string() };
        let inner: Vec<&str> = entries
            .iter()
            .copied()
            .filter(|e| fields.contains(&key(e)))
            .collect();
        if inner.is_empty() {
            continue;
        }
        let indent: String = body
            .trim_start_matches(['\n'])
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let nested = format!("{field}: {helper}{{ {} }}", inner.join(", "));
        let mut all: Vec<String> = Vec::new();
        let mut placed = false;
        for e in &entries {
            if fields.contains(&key(e)) {
                if !placed {
                    all.push(nested.clone());
                    placed = true;
                }
            } else {
                all.push(e.to_string());
            }
        }
        let new_body = if body.contains('\n') {
            let close_indent: String = text[..close]
                .rsplit('\n')
                .next()
                .unwrap_or("")
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            format!(
                "\n{indent}{},\n{close_indent}",
                all.join(&format!(",\n{indent}"))
            )
        } else {
            format!(" {} ", all.join(", "))
        };
        edits.push((open + 1, close, new_body));
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = text.to_string();
    for (s, e, t) in edits {
        out.replace_range(s..e, &t);
    }
    out
}
