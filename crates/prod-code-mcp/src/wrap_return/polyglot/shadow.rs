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

fn find_matching_open_paren(text: &str, close_paren: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, c) in text[..=close_paren].char_indices().rev() {
        if c == ')' {
            depth += 1;
        } else if c == '(' {
            depth -= 1;
            if depth == 0 {
                return Some(idx);
            }
        }
    }
    None
}

fn params_declare_name(params: &str, name: &str, lang: Language) -> bool {
    let mut search = params;
    while let Some(o) = search.find('{') {
        if let Some(c) = search[o..].find('}') {
            let inner = &search[o + 1..o + c];
            for field in inner.split(',') {
                let field = field.trim();
                let ident = if let Some((_, local)) = field.split_once(':') {
                    local.trim()
                } else {
                    field
                };
                let ident = ident.split('=').next().unwrap_or(ident).trim();
                let ident = ident.trim_start_matches("mut ").trim();
                if ident == name {
                    return true;
                }
            }
            search = &search[o + c + 1..];
        } else {
            break;
        }
    }

    for chunk in params.split(',') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        match lang {
            Language::TypeScript | Language::JavaScript | Language::Swift | Language::Rust => {
                let ident = chunk.split(':').next().unwrap_or(chunk);
                let ident = ident.split('=').next().unwrap_or(ident).trim();
                let ident = ident.trim_start_matches("mut ").trim();
                if ident == name {
                    return true;
                }
            }
            Language::Go => {
                let words: Vec<&str> = chunk.split_whitespace().collect();
                if words.first() == Some(&name) {
                    return true;
                }
            }
            _ => {
                let words: Vec<&str> = chunk.split_whitespace().collect();
                if words.contains(&name) {
                    return true;
                }
            }
        }
    }
    false
}

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

fn strip_closed_blocks(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text.as_bytes()[i] == b'{' {
            if is_lexical_block(text, i) {
                if let Some(close) = crate::parameter_object::matching_bracket(text, i) {
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
    let active_body = strip_closed_blocks(body_prefix);
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
                    if params_declare_name(rest, name, lang) {
                        return true;
                    }
                }
                current_decl = None;
            }
            continue;
        }

        match lang {
            Language::TypeScript | Language::JavaScript => {
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
                        if params_declare_name(rest, name, lang) {
                            return true;
                        }
                    }
                }
            }
            Language::Rust => {
                if let Some(rest) = trimmed.strip_prefix("let ") {
                    let rest = rest.trim_start_matches("mut ").trim();
                    if params_declare_name(rest, name, lang) {
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
            if params_declare_name(rest, name, lang) {
                return true;
            }
        }
    }
    false
}

fn is_python_shadowed(content: &str, at: usize, name: &str) -> bool {
    let lines: Vec<&str> = content[..at].lines().collect();
    let target_line = match lines.last() {
        Some(l) => *l,
        None => return false,
    };
    let target_indent = target_line.len() - target_line.trim_start().len();
    for (i, line) in lines.iter().enumerate().rev().skip(1) {
        let trimmed = line.trim();
        if trimmed.starts_with("def ") || trimmed.starts_with("async def ") {
            let indent = line.len() - line.trim_start().len();
            if indent < target_indent {
                let mut header = line.to_string();
                let mut body_start = i + 1;
                while !header.contains(')') && body_start < lines.len() {
                    header.push(' ');
                    header.push_str(lines[body_start].trim());
                    body_start += 1;
                }
                if let (Some(open), Some(close)) = (header.find('('), header.rfind(')')) {
                    let params = &header[open + 1..close];
                    for p in params.split(',') {
                        let p = p
                            .trim()
                            .split(':')
                            .next()
                            .unwrap()
                            .split('=')
                            .next()
                            .unwrap()
                            .trim();
                        if p == name {
                            return true;
                        }
                    }
                }
                let mut nested_fn_indent: Option<usize> = None;
                for body_line in &lines[body_start..lines.len() - 1] {
                    if body_line.trim().is_empty() {
                        continue;
                    }
                    let line_indent = body_line.len() - body_line.trim_start().len();
                    if let Some(fn_indent) = nested_fn_indent {
                        if line_indent > fn_indent {
                            continue;
                        } else {
                            nested_fn_indent = None;
                        }
                    }

                    let b_trimmed = body_line.trim();
                    if b_trimmed.starts_with("def ")
                        || b_trimmed.starts_with("async def ")
                        || b_trimmed.starts_with("class ")
                    {
                        nested_fn_indent = Some(line_indent);
                        continue;
                    }

                    if let Some((lhs, _)) = b_trimmed.split_once('=') {
                        let var = lhs.trim().split(':').next().unwrap().trim();
                        if var == name {
                            return true;
                        }
                    }
                }
                break;
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "shadow_tests.rs"]
mod tests;
