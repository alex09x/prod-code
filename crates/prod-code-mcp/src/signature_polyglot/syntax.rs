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

pub fn is_side_effect_free_argument(argument: &str, lang: Language) -> bool {
    let argument = if lang == Language::Python {
        argument
            .split_once('=')
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
    if argument.chars().all(is_ident) {
        return true;
    }
    if matches!(
        argument,
        "true" | "false" | "True" | "False" | "None" | "nil" | "null" | "nullptr"
    ) {
        return true;
    }
    let first = argument.chars().next().unwrap_or_default();
    if first.is_ascii_digit()
        || ((first == '-' || first == '+')
            && argument.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
    {
        return argument
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'));
    }
    if let Some(quote) = argument.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        return argument.ends_with(quote)
            && !argument[1..argument.len().saturating_sub(1)].contains('\n');
    }
    false
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
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_end = content[at..].find('\n').map_or(content.len(), |p| at + p);
    let line = content[line_start..line_end].trim();

    match lang {
        Language::Python => line.starts_with("import ") || line.starts_with("from "),
        Language::TypeScript | Language::JavaScript => {
            if line.starts_with("import ")
                || line.starts_with("import{")
                || line.contains(" from ")
                || line.contains("require(")
            {
                return true;
            }
            let search_start = at.saturating_sub(300);
            let before = &content[search_start..at];
            if let Some(imp_pos) = before.rfind("import ") {
                let between = &before[imp_pos..];
                if between.contains('{') && !between.contains('}') {
                    let search_end = content.len().min(at + 300);
                    let after = &content[at..search_end];
                    if after.contains('}') {
                        return true;
                    }
                }
            }
            false
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
                let paren = after.find('(').or_else(|| after.find('<'))?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
        }
        Language::TypeScript | Language::JavaScript => {
            if let Some(pos) = trimmed.find("function ") {
                let after = &trimmed[pos + 9..];
                let paren = after.find('(').or_else(|| after.find('<'))?;
                let name = after[..paren].trim();
                return Some(name.to_string());
            }
            if let Some(paren) = trimmed.find('(') {
                let before = trimmed[..paren].trim();
                if let Some(name) = before.split_whitespace().last() {
                    let clean = name.trim_end_matches('<');
                    if clean.chars().all(is_ident)
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
                if let Some(paren) = rest.find('(').or_else(|| rest.find('[')) {
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
