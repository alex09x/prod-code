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

use crate::parameter_object::Language;

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Python's floor division is not a C-style line comment. Keep the shared
/// parser for other languages, but use Python trivia rules for Python calls.
pub fn call_args_span(content: &str, after_name: usize, lang: Language) -> Option<(usize, usize)> {
    if lang != Language::Python {
        return crate::parameter_object::call_args_span(content, after_name);
    }
    let open = after_name + content[after_name..].len() - content[after_name..].trim_start().len();
    let bytes = content.as_bytes();
    if bytes.get(open) != Some(&b'(') {
        return None;
    }
    let mut stack = Vec::new();
    let mut at = open;
    while at < bytes.len() {
        let byte = bytes[at];
        if byte == b'#' {
            at += content[at..].find('\n').unwrap_or(bytes.len() - at);
            continue;
        }
        if matches!(byte, b'\'' | b'"') {
            let triple = bytes.get(at..at + 3) == Some(&[byte; 3]);
            at += if triple { 3 } else { 1 };
            loop {
                let next = *bytes.get(at)?;
                if next == b'\\' {
                    at += 2;
                } else if next == byte && (!triple || bytes.get(at..at + 3) == Some(&[byte; 3])) {
                    at += if triple { 3 } else { 1 };
                    break;
                } else {
                    at += 1;
                }
            }
            continue;
        }
        match byte {
            b'(' | b'[' | b'{' => stack.push(byte),
            b')' | b']' | b'}' => {
                if !matches!(
                    (stack.pop()?, byte),
                    (b'(', b')') | (b'[', b']') | (b'{', b'}')
                ) {
                    return None;
                }
                if stack.is_empty() {
                    return Some((open + 1, at));
                }
            }
            _ => {}
        }
        at += content[at..].chars().next()?.len_utf8();
    }
    None
}

pub fn is_side_effect_free_argument(argument: &str, lang: Language) -> bool {
    let argument = if lang == Language::Python {
        python_keyword(argument)
            .map(|(_, value)| value.trim())
            .unwrap_or(argument.trim())
    } else if lang == Language::Swift {
        argument
            .split_once(':')
            .filter(|(label, _)| label.trim().chars().all(is_ident))
            .map(|(_, value)| value.trim())
            .unwrap_or(argument.trim())
    } else {
        argument.trim()
    };
    if argument.is_empty() {
        return false;
    }
    if argument
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && argument.chars().all(is_ident)
        && !matches!(argument, "await" | "yield")
    {
        return true;
    }
    if matches!(
        argument,
        "true" | "false" | "True" | "False" | "None" | "nil" | "null" | "nullptr"
    ) {
        return true;
    }
    if super::numeric_literal::is_literal(argument, lang) {
        return true;
    }
    if let Some(quote) = argument.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        let mut escaped = false;
        for (offset, c) in argument.char_indices().skip(1) {
            if escaped {
                // Swift interpolation evaluates an expression inside a quoted token.
                if lang == Language::Swift && c == '(' {
                    return false;
                }
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == quote {
                return offset + c.len_utf8() == argument.len();
            } else if c == '\n' {
                return false;
            }
        }
        return false;
    }
    false
}

/// A keyword starts with a complete identifier followed by a single `=`.
/// Comparisons, walrus expressions, strings and nested expressions are values.
pub fn python_keyword(argument: &str) -> Option<(&str, &str)> {
    let argument = argument.trim();
    let first = argument.chars().next()?;
    if !(first.is_alphabetic() || first == '_') {
        return None;
    }
    let end = argument.find(|c| !is_ident(c)).unwrap_or(argument.len());
    let name = &argument[..end];
    let value = argument[end..].trim_start().strip_prefix('=')?;
    if value.starts_with('=') || value.trim().is_empty() {
        return None;
    }
    Some((name, value.trim()))
}

pub fn one_based_lsp_position(text: &str, byte_offset: usize) -> (u32, u32) {
    let before = &text[..byte_offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column_text = before.rsplit('\n').next().unwrap_or_default();
    let character = column_text.encode_utf16().count() as u32 + 1;
    (line, character)
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub fn is_in_comment(content: &str, at: usize, lang: Language) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let before_on_line = &content[line_start..at];
    let trimmed = before_on_line.trim_start();
    if lang == Language::Python {
        trimmed.starts_with('#') || before_on_line.contains('#')
    } else {
        trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed.starts_with('*')
            || before_on_line.contains("//")
    }
}

pub fn is_import_or_export_context(content: &str, at: usize, lang: Language) -> bool {
    is_import_or_export_context_with_jsx(content, at, lang, false)
}

pub fn is_import_or_export_context_with_jsx(
    content: &str,
    at: usize,
    lang: Language,
    jsx: bool,
) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_end = content[at..].find('\n').map_or(content.len(), |p| at + p);
    let line = content[line_start..line_end].trim();

    match lang {
        Language::Python => line.starts_with("import ") || line.starts_with("from "),
        Language::TypeScript | Language::JavaScript => {
            super::js_syntax::module_reference(content, at, lang, jsx)
        }
        Language::Cpp | Language::C => line.starts_with("#include") || line.starts_with("using "),
        Language::Swift => line.starts_with("import "),
        Language::Go => line.starts_with("import "),
        Language::Rust => line.starts_with("use "),
        Language::Java => line.starts_with("import ") || line.starts_with("package "),
    }
}

pub fn is_word_used(body: &str, word: &str) -> bool {
    for (at, _) in body.match_indices(word) {
        if at > 0 {
            let prev = body[..at].chars().next_back().unwrap();
            if is_ident(prev) {
                continue;
            }
        }
        let after = &body[at + word.len()..];
        if after.starts_with(is_ident) {
            continue;
        }
        // Exclude uses inside comments
        let line_start = body[..at].rfind('\n').map_or(0, |p| p + 1);
        let before_on_line = &body[line_start..at];
        if before_on_line.contains("//") || before_on_line.contains('#') {
            continue;
        }
        return true;
    }
    false
}

pub fn find_python_body_close(text: &str, def_offset: usize, colon_pos: usize) -> usize {
    let def_line = text[..def_offset].lines().last().unwrap_or("");
    let def_indent = def_line.len() - def_line.trim_start().len();
    let rest = &text[colon_pos + 1..];
    let mut current_offset = colon_pos + 1;
    for line in rest.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            current_offset += line.len() + 1;
            continue;
        }
        let line_indent = line.len() - line.trim_start().len();
        if line_indent <= def_indent {
            return current_offset;
        }
        current_offset += line.len() + 1;
    }
    text.len()
}

pub fn extract_decl_name_from_line(line: &str, lang: Language) -> Option<String> {
    let trimmed = line.trim();
    match lang {
        Language::Python => {
            if let Some(pos) = trimmed.find("def ") {
                let after = &trimmed[pos + 4..];
                let paren = after.find('(')?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::Swift => {
            if let Some(pos) = trimmed.find("func ") {
                let after = &trimmed[pos + 5..];
                // The name ends where its generic parameters or its parameter list begin.
                let paren = after.find(['(', '<'])?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if let Some(pos) = trimmed.find("function ") {
                let after = &trimmed[pos + 9..];
                let paren = after.find(['(', '<'])?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_end_matches('<');
                    if clean.chars().all(|c| is_ident(c) || c == '$')
                        && !clean.is_empty()
                        && !matches!(clean, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(clean.to_string());
                    }
                }
            }
        }
        Language::Go => {
            if let Some(pos) = trimmed.find("func ") {
                let after = &trimmed[pos + 5..];
                let rest = if after.starts_with('(') {
                    if let Some(close_recv) = after.find(')') {
                        after[close_recv + 1..].trim_start()
                    } else {
                        after
                    }
                } else {
                    after
                };
                if let Some(paren) = rest.find(['(', '[']) {
                    let name = rest[..paren].trim();
                    if name.chars().all(is_ident) && !name.is_empty() {
                        return Some(name.to_string());
                    }
                }
            }
        }
        Language::Cpp | Language::C | Language::Java => {
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_start_matches('*').trim_start_matches('&');
                    let member = clean.rsplit("::").next().unwrap_or(clean);
                    if member.chars().all(is_ident)
                        && !member.is_empty()
                        && !matches!(clean, "if" | "while" | "for" | "switch" | "catch")
                    {
                        return Some(member.to_string());
                    }
                }
            }
        }
        _ => {}
    }
    None
}
