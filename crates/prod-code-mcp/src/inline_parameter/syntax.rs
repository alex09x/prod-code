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
use std::path::Path;

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether an argument means the same in the callee's body as at the call: a literal, a
/// constant (`ALL_CAPS`), a type or unit value (`Mode`, `None`), or a path of those
/// (`Mode::Fast`, `crate::limits::MAX`, `Config.MAX`, `Math.PI`). A lowercase name may be a local of the caller; a call or
/// an expression may depend on one.
pub fn is_caller_independent(arg: &str) -> bool {
    let a = arg.trim();
    if a.is_empty() {
        return false;
    }
    // Disallow calls, indexings, expressions, borrows
    if a.contains('(')
        || a.contains(')')
        || a.contains('[')
        || a.contains(']')
        || a.contains('+')
        || a.contains('*')
        || a.contains('/')
        || a.contains('&')
        || a.contains('|')
        || a.contains('^')
        || a.contains('~')
        || a.contains('?')
    {
        return false;
    }
    if a.starts_with("self.")
        || a.starts_with("this.")
        || a.starts_with("self->")
        || a.starts_with("this->")
    {
        return false;
    }
    // Numbers
    let literal = a.strip_prefix('-').unwrap_or(a);
    if literal.starts_with(|c: char| c.is_ascii_digit())
        && literal.chars().all(|c| is_ident(c) || c == '.')
    {
        return true;
    }
    // Booleans & Nulls / Units
    if matches!(
        a,
        "true"
            | "false"
            | "True"
            | "False"
            | "None"
            | "nil"
            | "null"
            | "nullptr"
            | "undefined"
            | "Default"
    ) {
        return true;
    }
    // Strings & Chars
    if ((a.starts_with('"') && a.ends_with('"'))
        || (a.starts_with('\'') && a.ends_with('\''))
        || (a.starts_with("b\"") && a.ends_with('"')))
        && !a.contains('{')
    {
        return true;
    }
    // Path / Qualified names: separated by `::` or `.`
    let segments: Vec<&str> = if a.contains("::") {
        a.split("::").collect()
    } else {
        a.split('.').collect()
    };
    if segments
        .iter()
        .any(|s| s.is_empty() || !s.chars().all(is_ident))
    {
        return false;
    }
    if segments
        .first()
        .is_some_and(|&first| matches!(first, "self" | "this" | "super"))
    {
        return false;
    }
    let last = segments.last().copied().unwrap_or("");
    let first_char_upper = last.chars().next().is_some_and(char::is_uppercase);
    let all_caps = last
        .chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');

    all_caps || first_char_upper
}

pub fn keyword_arg(arg: &str) -> Option<(&str, &str)> {
    let trimmed = arg.trim();
    let name_len = trimmed.bytes().take_while(|b| is_ident(*b as char)).count();
    if name_len == 0 || trimmed.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let name = &trimmed[..name_len];
    let rest = trimmed[name_len..].trim_start();
    let value = rest.strip_prefix('=')?;
    if value.starts_with('=') {
        return None;
    }
    Some((name, value.trim()))
}

pub fn swift_label(arg: &str) -> Option<(&str, &str)> {
    let trimmed = arg.trim();
    let name_len = trimmed.bytes().take_while(|b| is_ident(*b as char)).count();
    if name_len == 0 || trimmed.as_bytes()[0].is_ascii_digit() {
        return None;
    }
    let name = &trimmed[..name_len];
    let rest = trimmed[name_len..].trim_start();
    let value = rest.strip_prefix(':')?;
    Some((name, value.trim()))
}

pub fn language_matches(lang: Language, path: &Path) -> bool {
    Language::of(path) == Some(lang)
        || (lang == Language::TypeScript && Language::of(path) == Some(Language::JavaScript))
        || (lang == Language::JavaScript && Language::of(path) == Some(Language::TypeScript))
        || (lang == Language::Cpp && Language::of(path) == Some(Language::C))
        || (lang == Language::C && Language::of(path) == Some(Language::Cpp))
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
                || line.starts_with("export ")
                || line.starts_with("export{")
                || line.contains(" from ")
                || line.contains("require(")
            {
                return true;
            }
            let search_start = at.saturating_sub(500);
            let before = &content[search_start..at];
            if let Some(imp_pos) = before.rfind("import ").or_else(|| before.rfind("export ")) {
                let after_keyword = before[imp_pos + 6..].trim_start();
                let after_keyword = after_keyword
                    .strip_prefix("type")
                    .unwrap_or(after_keyword)
                    .trim_start();
                if after_keyword.starts_with('{') {
                    let between = &before[imp_pos..];
                    if !between.contains('}') {
                        let search_end = content.len().min(at + 500);
                        let after = &content[at..search_end];
                        if after.contains('}') {
                            return true;
                        }
                    }
                }
            }
            false
        }
        Language::Go => {
            if line.starts_with("import ") {
                return true;
            }
            let search_start = at.saturating_sub(500);
            let before = &content[search_start..at];
            if let Some(imp_pos) = before.rfind("import (") {
                let between = &before[imp_pos..];
                if !between.contains(')') {
                    let search_end = content.len().min(at + 500);
                    let after = &content[at..search_end];
                    if after.contains(')') {
                        return true;
                    }
                }
            }
            false
        }
        Language::Cpp | Language::C => line.starts_with("#include") || line.starts_with("using "),
        Language::Swift => line.starts_with("import "),
        Language::Rust => {
            let without_pub = line
                .strip_prefix("pub ")
                .or_else(|| line.strip_prefix("pub(crate) "))
                .unwrap_or(line);
            without_pub.starts_with("use ")
        }
        Language::Java => line.starts_with("import ") || line.starts_with("package "),
    }
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

pub fn is_c_cpp_prototype(content: &str, at: usize, close_paren: usize) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let before_on_line = content[line_start..at].trim();
    let Some(before_word) = before_on_line.split_whitespace().last() else {
        return false;
    };
    let clean = before_word
        .trim_start_matches('*')
        .trim_start_matches('&')
        .trim_end_matches('*')
        .trim_end_matches('&');
    if clean.is_empty()
        || matches!(
            clean,
            "return" | "throw" | "case" | "goto" | "new" | "delete" | "co_return" | "co_yield"
        )
    {
        return false;
    }
    if !clean.chars().all(is_ident) {
        return false;
    }
    if close_paren + 1 > content.len() {
        return false;
    }
    let after_paren = content[close_paren + 1..].trim_start();
    after_paren.starts_with(';')
}

pub fn is_in_string(content: &str, at: usize, lang: Language) -> bool {
    let mut chars = content[..at].chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    while let Some(ch) = chars.next() {
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(delimiter) = quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == delimiter {
                quote = None;
            }
            continue;
        }
        if lang == Language::Python && ch == '#' {
            line_comment = true;
        } else if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            line_comment = true;
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            block_comment = true;
        } else if matches!(ch, '\'' | '"' | '`') {
            quote = Some(ch);
        }
    }
    quote.is_some() || line_comment || block_comment
}
