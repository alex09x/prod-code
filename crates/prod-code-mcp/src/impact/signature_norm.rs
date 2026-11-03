/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_syntax::{AstNode, Edition, SourceFile, TextRange, ast};

pub(crate) fn normalize_signature(lines: &[&str], language: &str) -> String {
    let mut parts = Vec::new();
    for line in lines {
        let mut trimmed = line.trim();
        if language == "python" {
            if let Some(pos) = trimmed.find('#') {
                trimmed = trimmed[..pos].trim();
            }
        } else if let Some(pos) = trimmed.find("//") {
            trimmed = trimmed[..pos].trim();
        }
        if !trimmed.is_empty() {
            parts.push(trimmed);
        }
    }
    let joined = parts.join(" ");
    let mut out = String::new();
    let mut prev_ws = false;
    for c in joined.chars() {
        if c.is_whitespace() {
            if !prev_ws {
                out.push(' ');
                prev_ws = true;
            }
        } else {
            out.push(c);
            prev_ws = false;
        }
    }
    let trimmed =
        out.trim_end_matches(|c: char| c == '{' || c == ':' || c == ';' || c.is_whitespace());
    let normalized = trimmed.replace("( ", "(").replace(" )", ")").to_string();

    if language == "rust" {
        normalize_rust_signature(&normalized)
    } else {
        normalized
    }
}

/// Normalizes Rust function signatures to ignore by-value parameter binding mutability (#1806).
///
/// In Rust, `mut x: T` and `x: T` have the exact same call signature and ABI: callers pass arguments
/// identically regardless of whether the parameter pattern binds with `mut`. However, meaningful
/// reference mutability (`&mut T`, `&mut self`) is part of the type/borrow contract and is preserved.
fn normalize_rust_signature(sig: &str) -> String {
    let wrapped = if sig.ends_with(';') {
        sig.to_string()
    } else {
        format!("{sig};")
    };
    let parsed = SourceFile::parse(&wrapped, Edition::CURRENT);
    let Some(fn_node) = parsed.tree().syntax().descendants().find_map(ast::Fn::cast) else {
        return sig.to_string();
    };
    let Some(param_list) = fn_node.param_list() else {
        return sig.to_string();
    };

    let mut mut_ranges: Vec<TextRange> = Vec::new();

    // 1. By-value self parameter (`mut self`, but not `&mut self`):
    if let Some(self_param) = param_list.self_param()
        && self_param.amp_token().is_none()
        && let Some(mut_token) = self_param.mut_token()
    {
        mut_ranges.push(mut_token.text_range());
    }

    // 2. Regular parameters: ignore `mut` on ident patterns (`mut items: Vec<Item>`).
    for param in param_list.params() {
        if let Some(pat) = param.pat() {
            for descendant in pat.syntax().descendants() {
                if let Some(ident_pat) = ast::IdentPat::cast(descendant)
                    && let Some(mut_token) = ident_pat.mut_token()
                {
                    mut_ranges.push(mut_token.text_range());
                }
            }
        }
    }

    if mut_ranges.is_empty() {
        return sig.to_string();
    }

    mut_ranges.sort_by_key(|r| std::cmp::Reverse(r.start()));
    let mut result = wrapped;
    for range in mut_ranges {
        let start = usize::from(range.start());
        let mut end = usize::from(range.end());
        while end < result.len() && result.as_bytes()[end].is_ascii_whitespace() {
            end += 1;
        }
        if start <= end && end <= result.len() {
            result.replace_range(start..end, "");
        }
    }

    if result.ends_with(';') {
        result.pop();
    }
    result.trim_end().to_string()
}
