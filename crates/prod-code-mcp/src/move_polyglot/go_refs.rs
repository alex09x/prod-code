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

use anyhow::Result;

use super::specifiers::is_ident;

pub(crate) fn scan_go_prefix(content: &str, at: usize) -> (bool, Option<(char, usize)>) {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut chars = content[..at].char_indices().peekable();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    while let Some((idx, ch)) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek().map(|(_, c)| *c) == Some('/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(delim) = quote {
            if delim == '`' {
                if ch == '`' {
                    quote = None;
                }
            } else if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delim {
                quote = None;
            }
            continue;
        }
        if ch == '/' && chars.peek().map(|(_, c)| *c) == Some('/') {
            chars.next();
            line_comment = true;
        } else if ch == '/' && chars.peek().map(|(_, c)| *c) == Some('*') {
            chars.next();
            block_comment = true;
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        } else if matches!(ch, '(' | '[' | '{') {
            stack.push((ch, idx));
        } else if matches!(ch, ')' | ']' | '}') {
            stack.pop();
        }
    }
    (
        quote.is_some() || line_comment || block_comment,
        stack.pop(),
    )
}

pub(crate) fn is_in_go_string_or_comment(content: &str, at: usize) -> bool {
    scan_go_prefix(content, at).0
}

fn is_short_var_decl_lhs(after_on_line: &str) -> bool {
    let mut rest = after_on_line.trim_start();
    loop {
        if rest.starts_with(":=") {
            return true;
        }
        if let Some(after_comma) = rest.strip_prefix(',') {
            let after_ws = after_comma.trim_start();
            let ident_bytes: usize = after_ws
                .chars()
                .take_while(|&c| is_ident(c))
                .map(char::len_utf8)
                .sum();
            if ident_bytes == 0 {
                return false;
            }
            rest = after_ws[ident_bytes..].trim_start();
        } else {
            return false;
        }
    }
}

fn composite_literal_type(before_brace: &str) -> &str {
    let trimmed = before_brace.trim_end();
    let mut depth_brace = 0i32;
    let mut depth_bracket = 0i32;
    let mut start = 0usize;
    for (idx, ch) in trimmed.char_indices().rev() {
        match ch {
            '}' => depth_brace += 1,
            '{' => {
                if depth_brace > 0 {
                    depth_brace -= 1;
                } else {
                    start = idx + 1;
                    break;
                }
            }
            ']' => depth_bracket += 1,
            '[' => {
                if depth_bracket > 0 {
                    depth_bracket -= 1;
                } else {
                    start = idx + 1;
                    break;
                }
            }
            ';' | '(' | ')' | '=' | ':' | ',' | '&' if depth_brace == 0 && depth_bracket == 0 => {
                start = idx + ch.len_utf8();
                break;
            }
            _ => {}
        }
    }
    let candidate = trimmed[start..].trim();
    candidate
        .strip_prefix("return")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .map_or(candidate, str::trim)
}

fn is_go_value_before_colon(
    source_text: &str,
    before_on_line: &str,
    enclosing: Option<(char, usize)>,
    source_file: Option<&Path>,
    root: Option<&Path>,
) -> bool {
    if before_on_line.starts_with("case ") || before_on_line.contains("{ case ") {
        return true;
    }
    match enclosing {
        Some(('[', _)) => true,
        Some(('{', brace_idx)) => {
            let lit_type = composite_literal_type(&source_text[..brace_idx]);
            lit_type.starts_with("map[")
                || lit_type.starts_with('[')
                || super::go_imports::is_named_map_or_slice_type(
                    source_text,
                    lit_type,
                    source_file,
                    root,
                )
        }
        _ => false,
    }
}

pub(crate) fn find_go_type_rhs<'a>(text: &'a str, type_name: &str) -> Option<&'a str> {
    let skip_type_params = |after: &'a str| -> &'a str {
        let trimmed = after.trim_start();
        if let Some(inner) = trimmed.strip_prefix('[')
            && let Some(close) = inner.find(']')
        {
            let rest = &inner[close + 1..];
            if rest.starts_with(char::is_whitespace) || rest.trim_start().starts_with('=') {
                return rest;
            }
        }
        trimmed
    };
    let needle = format!("type {type_name}");
    for (idx, _) in text.match_indices(&needle) {
        if (idx > 0 && text[..idx].chars().next_back().is_some_and(is_ident))
            || is_in_go_string_or_comment(text, idx)
        {
            continue;
        }
        let after = &text[idx + needle.len()..];
        if !after.chars().next().is_some_and(is_ident) {
            return Some(skip_type_params(after));
        }
    }
    let mut line_offset = 0usize;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let at = line_offset + (line.len() - trimmed.len());
        line_offset += line.len();
        if let Some(after) = trimmed.strip_prefix(type_name)
            && !after.chars().next().is_some_and(is_ident)
            && let (false, Some(('(', paren_idx))) = scan_go_prefix(text, at)
            && text[..paren_idx].trim_end().ends_with("type")
        {
            return Some(skip_type_params(after));
        }
    }
    None
}

fn last_code_char(segment: &str) -> Option<char> {
    segment
        .lines()
        .map(|l| l.split("//").next().unwrap_or(l).trim_end())
        .rfind(|l| !l.is_empty())
        .and_then(|l| l.chars().next_back())
}

fn is_go_func_param_name(source_text: &str, open_paren: usize, at: usize, end: usize) -> bool {
    let hdr = source_text[..open_paren].trim_end();
    let is_func_sig = hdr.ends_with("func")
        || hdr
            .rfind("func ")
            .is_some_and(|f| !hdr[f..].contains('{') && !hdr[f..].contains(';'));
    if !is_func_sig {
        return false;
    }
    if let Some(prev_ch) = last_code_char(&source_text[open_paren + 1..at])
        && prev_ch != ','
    {
        return false;
    }
    let mut rest = source_text[end..].trim_start();
    while let Some(after_comma) = rest.strip_prefix(',') {
        let after_ws = after_comma.trim_start();
        let ident_bytes: usize = after_ws
            .chars()
            .take_while(|&c| is_ident(c))
            .map(char::len_utf8)
            .sum();
        if ident_bytes == 0 {
            return false;
        }
        rest = after_ws[ident_bytes..].trim_start();
    }
    rest.starts_with("...")
        || rest.starts_with('[')
        || rest.starts_with('*')
        || rest.starts_with("<-")
        || rest
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
}

#[cfg(test)]
pub(crate) fn rewrite_go_call_sites(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
) -> Result<(String, usize)> {
    rewrite_go_call_sites_in_context(source_text, decl_name, target_pkg, None, None)
}

pub(crate) fn rewrite_go_call_sites_in_context(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
    source_file: Option<&Path>,
    root: Option<&Path>,
) -> Result<(String, usize)> {
    if decl_name.is_empty() {
        return Ok((source_text.to_string(), 0));
    }
    let mut edits = Vec::new();
    for (at, _) in source_text.match_indices(decl_name) {
        let end = at + decl_name.len();
        if (at > 0 && source_text[..at].chars().next_back().is_some_and(is_ident))
            || source_text[end..].chars().next().is_some_and(is_ident)
        {
            continue;
        }
        let (in_str_or_comment, enclosing) = scan_go_prefix(source_text, at);
        if in_str_or_comment {
            continue;
        }
        let line_start = source_text[..at].rfind('\n').map_or(0, |i| i + 1);
        let line_end = source_text[at..]
            .find('\n')
            .map_or(source_text.len(), |i| at + i);
        let before_on_line = source_text[line_start..at].trim_start();
        let before = source_text[..at].trim_end();
        if before_on_line.starts_with("package ") || before.ends_with('.') {
            continue;
        }
        let after = source_text[end..].trim_start();
        let after_on_line = source_text[end..line_end].trim_start();

        if before_on_line.trim_end() == "func"
            || (before_on_line.starts_with("func ")
                && before_on_line
                    .trim_start_matches("func")
                    .trim_start()
                    .starts_with('(')
                && before_on_line.trim_end().ends_with(')')
                && after.starts_with('('))
        {
            continue;
        }

        let prev_word = before.rsplit(|c: char| !is_ident(c)).next().unwrap_or("");
        if matches!(prev_word, "var" | "const" | "type") {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is shadowed by a local `{prev_word}` declaration; cannot safely qualify it"
            );
        }

        if is_short_var_decl_lhs(after_on_line) {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is shadowed by a local binding; cannot safely qualify it"
            );
        }

        if let Some(('(', open_paren)) = enclosing
            && is_go_func_param_name(source_text, open_paren, at, end)
        {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is shadowed by a parameter declaration; cannot safely qualify it"
            );
        }

        if after_on_line.starts_with("...")
            || after_on_line.starts_with("[]")
            || after_on_line
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            if let Some(('{', brace_idx)) = enclosing
                && source_text[..brace_idx].trim_end().ends_with("struct")
            {
                continue;
            }
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is shadowed by a parameter declaration; cannot safely qualify it"
            );
        }

        if after.starts_with(':')
            && !after.starts_with(":=")
            && !is_go_value_before_colon(source_text, before_on_line, enclosing, source_file, root)
        {
            continue;
        }

        if !decl_name.chars().next().is_some_and(char::is_uppercase) {
            anyhow::bail!(
                "Go symbol `{decl_name}` at byte {at} is unexported and cannot be qualified across packages"
            );
        }

        edits.push((at, decl_name.len()));
    }
    let count = edits.len();
    let mut rewritten = source_text.to_string();
    for (at, len) in edits.into_iter().rev() {
        rewritten.replace_range(at..at + len, &format!("{target_pkg}.{decl_name}"));
    }
    Ok((rewritten, count))
}
