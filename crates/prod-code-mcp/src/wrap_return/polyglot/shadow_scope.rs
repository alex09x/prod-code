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

pub(super) fn find_matching_open_paren(text: &str, close_paren: usize) -> Option<usize> {
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

pub(super) fn params_declare_name(params: &str, name: &str, lang: Language) -> bool {
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
                let ident = ident.trim_end_matches(';').trim();
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

fn split_top_level(text: &str, separator: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    for (i, ch) in text.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            continue;
        }
        match ch {
            '(' | '[' | '{' => stack.push(ch),
            ')' | ']' | '}' => {
                stack.pop();
            }
            _ => {}
        }
        if ch == separator && stack.is_empty() {
            parts.push(&text[start..i]);
            start = i + ch.len_utf8();
        }
    }
    parts.push(&text[start..]);
    parts
}

fn binding_pattern(declaration: &str) -> &str {
    split_top_level(declaration, '=')
        .first()
        .copied()
        .unwrap_or(declaration)
        .trim()
}

pub(super) fn declaration_declares_name(declaration: &str, name: &str, lang: Language) -> bool {
    split_top_level(declaration, ',')
        .into_iter()
        .any(|binding| params_declare_name(binding_pattern(binding), name, lang))
}

pub(super) fn js_function_declaration_has_name(line: &str, name: &str) -> bool {
    let line = line.trim();
    let line = line.strip_prefix("async ").unwrap_or(line);
    let Some(rest) = line.strip_prefix("function") else {
        return false;
    };
    let rest = if let Some(rest) = rest.strip_prefix('*') {
        rest
    } else if rest.starts_with(char::is_whitespace) {
        rest
    } else {
        return false;
    }
    .trim_start();
    let declared: String = rest
        .chars()
        .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '$')
        .collect();
    declared == name && rest[declared.len()..].trim_start().starts_with('(')
}

pub(super) fn is_js_function_declaration_name(
    content: &str,
    at: usize,
    name: &str,
    lang: Language,
) -> bool {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return false;
    }
    let line_start = content[..at].rfind('\n').map_or(0, |i| i + 1);
    let prefix = content[line_start..at].trim_end();
    let after = content[at + name.len()..].trim_start();
    after.starts_with('(')
        && [
            "function",
            "function*",
            "function *",
            "async function",
            "async function*",
            "async function *",
        ]
        .iter()
        .any(|prefix_word| prefix.ends_with(prefix_word))
}

pub(super) fn is_shadowed_by_expression_arrow(
    content: &str,
    at: usize,
    name: &str,
    lang: Language,
) -> bool {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return false;
    }
    for (arrow_at, _) in content[..at].rmatch_indices("=>") {
        if crate::inline_parameter::is_in_comment(content, arrow_at, lang)
            || crate::inline_parameter::is_in_string(content, arrow_at, lang)
        {
            continue;
        }
        let body_prefix = &content[arrow_at + 2..at];
        if arrow_expression_finished(body_prefix) {
            continue;
        }
        let header = content[..arrow_at].trim_end();
        if header.ends_with(')') {
            let close = header.len() - 1;
            if let Some(open) = find_matching_open_paren(header, close)
                && params_declare_name(&header[open + 1..close], name, lang)
            {
                return true;
            }
        } else {
            let parameter: String = header
                .chars()
                .rev()
                .take_while(|ch| ch.is_alphanumeric() || *ch == '_' || *ch == '$')
                .collect::<String>()
                .chars()
                .rev()
                .collect();
            if parameter == name {
                return true;
            }
        }
    }
    false
}

fn arrow_expression_finished(prefix: &str) -> bool {
    let mut stack = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut expression_started = false;
    for (index, ch) in prefix.char_indices() {
        if let Some(active) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == active {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
            continue;
        }
        match ch {
            '(' | '[' | '{' => stack.push(ch),
            ')' | ']' | '}' => {
                if stack.is_empty() {
                    return true;
                }
                stack.pop();
            }
            _ => {}
        }
        if stack.is_empty() {
            if matches!(ch, ',' | ';')
                || (ch == '\n'
                    && expression_started
                    && !line_break_continues_expression(prefix, index))
            {
                return true;
            }
            if !ch.is_whitespace() {
                expression_started = true;
            }
        }
    }
    false
}

fn line_break_continues_expression(prefix: &str, newline_at: usize) -> bool {
    let before = prefix[..newline_at].trim_end();
    let after = prefix[newline_at + 1..].trim_start();
    let continues_after = [
        "?", ":", ".", "?.", "??", "||", "&&", "+", "-", "*", "/", "%", "|", "&", "^", "=", "<",
        ">", "(", "[",
    ]
    .iter()
    .any(|operator| after.starts_with(operator));
    let continues_before = [
        "?", ":", ".", "?.", "??", "||", "&&", "+", "-", "*", "/", "%", "|", "&", "^", "=", "<",
        ">",
    ]
    .iter()
    .any(|operator| before.ends_with(operator));
    continues_after || continues_before
}
