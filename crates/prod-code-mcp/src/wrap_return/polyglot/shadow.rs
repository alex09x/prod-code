/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::Language;

/// Checks if `name` at byte offset `at` is shadowed by an enclosing local variable or parameter.
/// Used during empty-reference fallback to prevent rewriting calls to local parameters/bindings (#985).
pub(crate) fn is_locally_shadowed(content: &str, at: usize, name: &str, lang: Language) -> bool {
    if lang == Language::Python {
        return is_python_shadowed(content, at, name);
    }
    if is_js_function_declaration_name(content, at, name, lang) {
        return true;
    }
    if is_shadowed_by_expression_arrow(content, at, name, lang) {
        return true;
    }

    // Check if `at` itself is inside a parameter list of a function header:
    if let Some(open_p) = content[..at].rfind('(') {
        if let Some(close_p) = crate::parameter_object::matching_bracket(content, open_p) {
            if open_p < at && at < close_p {
                let after_paren = content[close_p + 1..].trim_start();
                if let Some(open_b) = after_paren.find('{') {
                    if open_b < 200 && !after_paren[..open_b].contains(';') {
                        let params = &content[open_p + 1..close_p];
                        if params_declare_name(params, name, lang) {
                            return true;
                        }
                    }
                }
            }
        }
    }

    // Check if `at` itself is a local variable declaration (const name =, let name =, etc.)
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_before = content[line_start..at].trim();
    if let Some(rest) = line_before
        .strip_prefix("const ")
        .or_else(|| line_before.strip_prefix("let "))
        .or_else(|| line_before.strip_prefix("var "))
        .or_else(|| line_before.strip_prefix("mut "))
    {
        if !rest.contains('=') && (rest.trim().is_empty() || rest.starts_with('{')) {
            return true;
        }
    } else if content[at + name.len()..].trim_start().starts_with(":=") {
        return true;
    }

    // Bracket-scoped languages (TS, JS, Go, Rust, C, C++, Swift)
    let mut search = at;
    while let Some(open_brace) = content[..search].rfind('{') {
        search = open_brace;
        let Some(close_brace) = crate::parameter_object::matching_bracket(content, open_brace)
        else {
            continue;
        };
        if open_brace < at && at < close_brace {
            // 1. Check local variable declarations between open_brace and at
            let body_prefix = &content[open_brace + 1..at];
            if body_has_local_decl(body_prefix, name, lang) {
                return true;
            }

            // 2. Check parameter list before open_brace
            if let Some(close_paren) = content[..open_brace].rfind(')') {
                let between = &content[close_paren + 1..open_brace];
                if between.len() < 200 {
                    if let Some(open_paren) = find_matching_open_paren(content, close_paren) {
                        let params = &content[open_paren + 1..close_paren];
                        if params_declare_name(params, name, lang) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    false
}

#[path = "shadow_scope.rs"]
mod shadow_scope;
use shadow_scope::{
    declaration_declares_name, find_matching_open_paren, is_js_function_declaration_name,
    is_shadowed_by_expression_arrow, js_function_declaration_has_name, params_declare_name,
};

fn is_lexical_block(text: &str, open_pos: usize) -> bool {
    let before = text[..open_pos].trim_end();
    if before.is_empty() {
        return true;
    }

    if before.ends_with('=')
        || before.ends_with(':')
        || before.ends_with(',')
        || before.ends_with('(')
        || before.ends_with('[')
    {
        return false;
    }

    let line_start = before.rfind('\n').map_or(0, |p| p + 1);
    let line_before = before[line_start..].trim();

    if line_before.starts_with("const ")
        || line_before.starts_with("let ")
        || line_before.starts_with("var ")
        || line_before.starts_with("val ")
        || line_before.starts_with("type ")
        || line_before.starts_with("return ")
        || line_before == "const"
        || line_before == "let"
        || line_before == "var"
        || line_before == "val"
        || line_before == "return"
    {
        return false;
    }

    if before.ends_with("=>") {
        return true;
    }

    if before.ends_with(';') || before.ends_with('{') || before.ends_with('}') {
        return true;
    }

    if before.ends_with(')') {
        return true;
    }

    let last_word = before
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next_back()
        .unwrap_or("");
    if matches!(
        last_word,
        "else" | "do" | "try" | "finally" | "catch" | "unsafe" | "loop" | "select"
    ) {
        return true;
    }

    if line_before.starts_with("if ")
        || line_before.starts_with("for ")
        || line_before.starts_with("while ")
        || line_before.starts_with("switch ")
        || line_before.starts_with("match ")
    {
        return true;
    }

    line_before.is_empty()
}

fn extract_var_decls(block: &str) -> String {
    let mut out = String::new();
    for line in block.lines() {
        let trimmed = line.trim();
        if let Some(idx) = trimmed.find("var ") {
            let before = &trimmed[..idx];
            let prev = before.chars().next_back();
            if prev.map_or(true, |c| !c.is_alphanumeric() && c != '_' && c != '$') {
                let decl = &trimmed[idx..];
                let decl = if let Some(semi) = decl.find(';') {
                    &decl[..=semi]
                } else {
                    decl
                };
                out.push_str(decl.trim());
                out.push('\n');
            }
        }
    }
    out
}

fn strip_closed_blocks(text: &str, lang: Language) -> String {
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text.as_bytes()[i] == b'{' {
            if is_lexical_block(text, i) {
                if let Some(close) = crate::parameter_object::matching_bracket(text, i) {
                    if matches!(lang, Language::TypeScript | Language::JavaScript)
                        && is_ordinary_js_block(text, i)
                    {
                        let inner = &text[i + 1..close];
                        let non_fn_inner = strip_function_blocks(inner);
                        let vars = extract_var_decls(&non_fn_inner);
                        if !vars.is_empty() {
                            result.push('\n');
                            result.push_str(&vars);
                        }
                    }
                    result.push('\n');
                    i = close + 1;
                    continue;
                }
            }
        }
        let ch = text[i..].chars().next().unwrap();
        result.push(ch);
        i += ch.len_utf8();
    }
    result
}

fn body_has_local_decl(body_prefix: &str, name: &str, lang: Language) -> bool {
    let active_body = strip_closed_blocks(body_prefix, lang);
    let mut current_decl: Option<String> = None;
    for line in active_body.lines() {
        let trimmed = line.trim();
        if let Some(ref mut decl) = current_decl {
            decl.push(' ');
            decl.push_str(trimmed);
            if trimmed.contains(';') || trimmed.contains('=') {
                if let Some(rest) = decl
                    .strip_prefix("const ")
                    .or_else(|| decl.strip_prefix("let "))
                    .or_else(|| decl.strip_prefix("var "))
                {
                    if declaration_declares_name(rest, name, lang) {
                        return true;
                    }
                }
                current_decl = None;
            }
            continue;
        }

        match lang {
            Language::TypeScript | Language::JavaScript => {
                if js_function_declaration_has_name(trimmed, name) {
                    return true;
                }
                if trimmed.starts_with("const ")
                    || trimmed.starts_with("let ")
                    || trimmed.starts_with("var ")
                {
                    if !trimmed.contains(';') && !trimmed.contains('=') {
                        current_decl = Some(trimmed.to_string());
                        continue;
                    }
                    if let Some(rest) = trimmed
                        .strip_prefix("const ")
                        .or_else(|| trimmed.strip_prefix("let "))
                        .or_else(|| trimmed.strip_prefix("var "))
                    {
                        if declaration_declares_name(rest, name, lang) {
                            return true;
                        }
                    }
                }
            }
            Language::Rust => {
                if let Some(rest) = trimmed.strip_prefix("let ") {
                    let rest = rest.trim_start_matches("mut ").trim();
                    if declaration_declares_name(rest, name, lang) {
                        return true;
                    }
                }
            }
            Language::Go => {
                if let Some((lhs, _)) = trimmed.split_once(":=") {
                    for v in lhs.split(',') {
                        if v.trim() == name {
                            return true;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(decl) = current_decl {
        if let Some(rest) = decl
            .strip_prefix("const ")
            .or_else(|| decl.strip_prefix("let "))
            .or_else(|| decl.strip_prefix("var "))
        {
            if declaration_declares_name(rest, name, lang) {
                return true;
            }
        }
    }
    false
}

#[path = "shadow_python.rs"]
mod shadow_python;
use shadow_python::is_python_shadowed;

#[path = "shadow_js.rs"]
mod shadow_js;
use shadow_js::{is_ordinary_js_block, strip_function_blocks};

#[cfg(test)]
#[path = "shadow_tests.rs"]
mod tests;
