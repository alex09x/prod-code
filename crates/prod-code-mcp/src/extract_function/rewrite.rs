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
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::tokens::is_ident;
use super::types::{PLACEHOLDER, Rewrite};

/// Reads [`Rewrite`] off the text before and after rust-analyzer extracted `old[start..end]`
/// into `fun_name`.
pub fn rewrite_of(old: &str, new: &str, start: usize, end: usize) -> Option<Rewrite> {
    if !old.is_char_boundary(start) || !new.is_char_boundary(start) || old[..start] != new[..start]
    {
        return None;
    }
    let def = new
        .match_indices(&format!("fn {PLACEHOLDER}"))
        .map(|(i, _)| i)
        .find(|i| {
            !new[i + 3 + PLACEHOLDER.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
        })?;
    if def < start {
        return None;
    }
    let open = def + new[def..].find('{')?;
    let function_end = crate::parameter_object::matching_bracket(new, open)? + 1;
    // After the new function the two texts agree to the end.
    let tail = new.len() - function_end;
    if tail > old.len() || old[old.len() - tail..] != new[function_end..] {
        return None;
    }
    let inserted_at = old.len() - tail;
    if inserted_at < end {
        return None;
    }
    let rest = &old[end..inserted_at];
    // The new function starts after the rest, with whatever blank lines and modifiers precede
    // `fn`: the latest point before the definition's line where the rest ends.
    let def_line = new[..def].rfind('\n').map_or(0, |i| i + 1);
    let function_start = (start..=def_line)
        .rev()
        .filter(|x| new.is_char_boundary(*x))
        .find(|x| new[..*x].ends_with(rest) && *x >= start + rest.len())?;
    let call = new[start..function_start - rest.len()].to_string();
    let rewrite = Rewrite {
        call,
        inserted_at,
        function_len: function_end - function_start,
    };
    (!rewrite.call.trim().is_empty()).then_some(rewrite)
}

/// The indentation of the line that holds `at`.
pub fn indent_at(text: &str, at: usize) -> &str {
    let line = text[..at].rfind('\n').map_or(0, |i| i + 1);
    let width = text[line..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .map(char::len_utf8)
        .sum::<usize>();
    &text[line..line + width]
}

/// `text` with every `fun_name` name token made `name`: one in a string or a comment stays as
/// it is (#1439).
pub fn rename_placeholder(text: &str, name: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (kind, s, e) in super::tokens::tokens(text) {
        if kind == super::tokens::Token::Word && &text[s..e] == PLACEHOLDER {
            out.push_str(&text[at..s]);
            out.push_str(name);
            at = e;
        }
    }
    out.push_str(&text[at..]);
    out
}

/// `preferred`, or `preferred_2`, `preferred_3`, …: the first that no name token of `code` and
/// no name in `taken` already spells, so a parameter that replaces a literal is never shadowed
/// by a local of the same name in the helper's body (#1439).
pub fn fresh_name(code: &str, preferred: &str, taken: &[String]) -> String {
    let words: std::collections::BTreeSet<&str> = super::tokens::tokens(code)
        .into_iter()
        .filter(|(kind, ..)| *kind == super::tokens::Token::Word)
        .map(|(_, s, e)| &code[s..e])
        .collect();
    let free = |name: &str| !words.contains(name) && !taken.iter().any(|t| t == name);
    if free(preferred) {
        return preferred.to_string();
    }
    (2..)
        .map(|k| format!("{preferred}_{k}"))
        .find(|name| free(name))
        .expect("an unused suffix exists")
}

/// Another file, `original`, with the calls of `edits` put in and named `name`. Only the calls
/// are renamed: the file's own text, even a `fun_name` of its own, stays as it was (#1439).
pub fn with_calls(original: &str, edits: &[(usize, usize, String)], name: &str) -> String {
    let named: Vec<(usize, usize, String)> = edits
        .iter()
        .map(|(from, to, call)| (*from, *to, rename_placeholder(call, name)))
        .collect();
    apply_edits(original, &named)
}

/// The type in a hover on a literal: the first code block, when it is one line that reads like
/// a type (`u32`, `&str`, `f64`).
pub fn literal_type(hover: &str) -> Option<String> {
    let block = hover.split("```").nth(1)?;
    let body = block.strip_prefix("rust").unwrap_or(block).trim();
    let one_line = !body.contains('\n') && !body.is_empty();
    let looks_like_type = body.chars().all(|c| {
        c.is_alphanumeric() || matches!(c, '_' | '&' | '\'' | ':' | '<' | '>' | ' ' | ',')
    }) && !body.contains("fn ")
        && !body.starts_with("let ");
    (one_line && looks_like_type).then(|| body.to_string())
}

/// `text` with every edit (byte range of `text`, replacement) applied; the edits do not overlap.
pub fn apply_edits(text: &str, edits: &[(usize, usize, String)]) -> String {
    let mut sorted = edits.to_vec();
    sorted.sort_by_key(|(from, _, _)| std::cmp::Reverse(*from));
    let mut out = text.to_string();
    for (from, to, replacement) in sorted {
        out.replace_range(from..to, &replacement);
    }
    out
}

/// `call` (`let net = fun_name(o);`) with `extra` appended to the arguments of `fun_name`.
pub fn with_arguments(call: &str, extra: &[String]) -> Option<String> {
    if extra.is_empty() {
        return Some(call.to_string());
    }
    let at = call.find(&format!("{PLACEHOLDER}("))? + PLACEHOLDER.len();
    let close = crate::parameter_object::matching_bracket(call, at)?;
    let inside = call[at + 1..close].trim();
    let joined = extra.join(", ");
    let args = if inside.is_empty() {
        joined
    } else {
        format!("{inside}, {joined}")
    };
    Some(format!("{}{args}{}", &call[..at + 1], &call[close..]))
}

/// The errors the analyzer finds in `files` checked together.
pub async fn errors_in(
    remote: SocketAddr,
    root: &Path,
    files: &[(PathBuf, String)],
) -> Result<Vec<String>> {
    let reports = crate::diagnostics::validate_texts(remote, root, files, &[]).await?;
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

#[cfg(test)]
#[path = "rewrite_tests.rs"]
mod tests;
