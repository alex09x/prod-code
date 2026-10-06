/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::conversion::into_call;
use super::types::Site;
use crate::parameter_object::Language;
use std::path::Path;

/// The span of the type in a declaration, given the offset of the declared name.
///
/// Four shapes cover what can be migrated: a field or a parameter or an annotated `let`, which
/// are `name: Type` and end at the first `,`, `)`, `;` or `=` that is not inside brackets; and
/// a function, whose type is what follows `->`.
pub fn declared_type_span(text: &str, name_offset: usize) -> Option<(usize, usize)> {
    declared_type_span_polyglot(text, name_offset, Language::Rust)
}

/// The span of the type in a declaration across polyglot languages (Rust, TypeScript/JavaScript,
/// Python, C++, Swift, Go).
pub fn declared_type_span_polyglot(
    text: &str,
    name_offset: usize,
    lang: Language,
) -> Option<(usize, usize)> {
    let bytes = text.as_bytes();
    let mut i = name_offset;
    while i < bytes.len() && (bytes[i] == b'_' || (bytes[i] as char).is_alphanumeric()) {
        i += 1;
    }
    let after_name = i;
    while i < bytes.len() && (bytes[i] as char).is_whitespace() {
        i += 1;
    }

    // Function return type
    if bytes.get(i) == Some(&b'(') {
        let close = matching(text, i)?;
        match lang {
            Language::Rust | Language::Swift => {
                let arrow = text[close..].find("->")? + close;
                let body = text[close..].find(['{', ';']).map(|b| b + close);
                if body.is_some_and(|b| b < arrow) {
                    return None;
                }
                let start = arrow + 2;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = end_of_type(text, start, b"{;\n")?;
                return Some((start, end));
            }
            Language::Python => {
                let colon = text[close..].find(':')? + close;
                let between = &text[close..colon];
                let arrow = between.find("->")? + close;
                let start = arrow + 2;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = colon - (text[start..colon].len() - text[start..colon].trim_end().len());
                return Some((start, end));
            }
            Language::TypeScript | Language::JavaScript => {
                let body = text[close..].find(['{', ';']).map(|b| b + close);
                let arrow = text[close..].find("=>").map(|b| b + close);
                let end_header = match (body, arrow) {
                    (Some(b), Some(a)) => b.min(a),
                    (Some(b), None) => b,
                    (None, Some(a)) => a,
                    (None, None) => return None,
                };
                let colon = text[close..end_header].find(':')? + close;
                let start = colon + 1;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = end_of_type(text, start, b"{;=\n")?;
                return Some((start, end));
            }
            Language::Go => {
                let body = text[close..].find('{')? + close;
                let header = text[close + 1..body].trim();
                if header.is_empty() {
                    return None;
                }
                let start = close
                    + 1
                    + (text[close + 1..body].len() - text[close + 1..body].trim_start().len());
                let end =
                    body - (text[close + 1..body].len() - text[close + 1..body].trim_end().len());
                return Some((start, end));
            }
            Language::Cpp | Language::C | Language::Java => {
                // In C/C++, return type is before function name
                let line_start = text[..name_offset]
                    .rfind(['\n', ';', '{', '}'])
                    .map_or(0, |p| p + 1);
                let before = text[line_start..name_offset].trim();
                let words: Vec<&str> = before.split_whitespace().collect();
                if words.is_empty() {
                    return None;
                }
                let filtered: Vec<&str> = words
                    .into_iter()
                    .filter(|w| {
                        !matches!(*w, "virtual" | "static" | "inline" | "constexpr" | "friend")
                    })
                    .collect();
                if filtered.is_empty() {
                    return None;
                }
                let start = text[line_start..name_offset].find(filtered[0])? + line_start;
                let last = filtered.last().unwrap();
                let end_rel = text[start..name_offset].rfind(last)? + last.len();
                return Some((start, start + end_rel));
            }
        }
    }

    // Parameters, fields, and variables
    match lang {
        Language::Rust
        | Language::Swift
        | Language::TypeScript
        | Language::JavaScript
        | Language::Python => {
            let mut check_pos = after_name;
            while check_pos < bytes.len() && (bytes[check_pos] as char).is_whitespace() {
                check_pos += 1;
            }
            if bytes.get(check_pos) == Some(&b'?') {
                check_pos += 1;
                while check_pos < bytes.len() && (bytes[check_pos] as char).is_whitespace() {
                    check_pos += 1;
                }
            }
            if bytes.get(check_pos) == Some(&b':') {
                let start = check_pos + 1;
                let start = start + text[start..].len() - text[start..].trim_start().len();
                let end = end_of_type(text, start, b",);=\n#")?;
                return Some((start, end));
            }
            None
        }
        Language::Go => {
            if i < bytes.len()
                && !matches!(bytes[i], b'=' | b':' | b',' | b')' | b'{' | b';' | b'\n')
            {
                let start = i;
                let end = end_of_type(text, start, b",);=\n{`")?;
                return Some((start, end));
            }
            None
        }
        Language::Cpp | Language::C | Language::Java => {
            let line_start = text[..name_offset]
                .rfind(['\n', ';', '{', '}', '(', ','])
                .map_or(0, |p| p + 1);
            let before = text[line_start..name_offset].trim();
            if before.is_empty() {
                return None;
            }
            let words: Vec<&str> = before.split_whitespace().collect();
            let filtered: Vec<&str> = words
                .into_iter()
                .filter(|w| {
                    !matches!(
                        *w,
                        "auto"
                            | "register"
                            | "static"
                            | "extern"
                            | "public:"
                            | "private:"
                            | "protected:"
                    )
                })
                .collect();
            if filtered.is_empty() {
                return None;
            }
            let start = text[line_start..name_offset].find(filtered[0])? + line_start;
            let last = filtered.last().unwrap();
            let end_rel = text[start..name_offset].rfind(last)? + last.len();
            Some((start, start + end_rel))
        }
    }
}

/// The offset just past the `)` that closes the `(` at `open`.
pub(crate) fn matching(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    for (i, c) in bytes.iter().enumerate().skip(open) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Where a type written at `start` ends: the first terminator at bracket depth zero.
pub(crate) fn end_of_type(text: &str, start: usize, terminators: &[u8]) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = start;
    while i < bytes.len() {
        let c = bytes[i];
        let prev = if i > 0 { bytes[i - 1] } else { b' ' };
        let next = bytes.get(i + 1).copied().unwrap_or(b' ');
        // The terminator is read before the depth is touched: `{` ends a return type and also
        // opens a block, and deciding in the other order loses every function's type.
        if depth == 0 && terminators.contains(&c) && !(c == b'=' && (next == b'=' || next == b'>'))
        {
            return Some(text[..i].trim_end().len());
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            // `<` and `>` nest a type, but `->` and `=>` are not brackets.
            b'<' if prev != b'-' && prev != b'=' => depth += 1,
            b'>' if prev != b'-' && prev != b'=' && depth > 0 => depth -= 1,
            _ => {}
        }
        if depth == 0 && c == b'\n' && terminators.contains(&b',') {
            // A field or parameter written without its trailing comma still ends at its line.
            return Some(text[..i].trim_end().len());
        }
        i += 1;
    }
    None
}

/// A site's expression in its file's text, as a byte range.
pub fn expression_span(text: &str, site: &Site) -> Option<(usize, usize)> {
    let start = crate::signature::offset_of(text, site.line, site.col)?;
    let mut end = if let Some((end_line, end_col)) = site.end {
        crate::signature::offset_of(text, end_line, end_col)?
    } else {
        let line_rest = &text[start..];
        let token_len: usize = line_rest
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
            .map(|c| c.len_utf8())
            .sum();
        if token_len == 0 {
            return None;
        }
        start + token_len
    };
    if text[end..].starts_with('(') {
        let mut depth = 0i32;
        for (i, c) in text[end..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end += i + 1;
                        break;
                    }
                }
                '\n' => return None,
                _ => {}
            }
        }
    }
    let expr = text.get(start..end)?;
    if start >= end || expr.contains('\n') || expr.trim().is_empty() {
        return None;
    }
    let file_lang = Language::of(Path::new(&site.file)).unwrap_or(Language::Rust);
    if file_lang == Language::Rust
        && text[..start].ends_with('.')
        && into_call(expr).starts_with('(')
    {
        return None;
    }
    Some((start, end))
}

pub(crate) fn is_import_line(content: &str, at: usize, lang: Language) -> bool {
    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let line_end = content[at..].find('\n').map_or(content.len(), |p| at + p);
    let line = content[line_start..line_end].trim();
    match lang {
        Language::Python => line.starts_with("import ") || line.starts_with("from "),
        Language::TypeScript | Language::JavaScript => {
            line.starts_with("import ")
                || line.starts_with("import{")
                || line.contains(" from ")
                || line.contains("require(")
        }
        Language::Go => line.starts_with("import ") || line.starts_with("import ("),
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

pub fn find_symbol_decl_offset(
    text: &str,
    clean_name: &str,
    lang: Language,
    prefer_line: Option<u32>,
) -> Option<usize> {
    if let Some(l) = prefer_line {
        let lines: Vec<&str> = text.lines().collect();
        if l > 0 && (l as usize) <= lines.len() {
            let target_idx = (l - 1) as usize;
            let start_idx = target_idx.saturating_sub(2);
            let end_idx = (target_idx + 2).min(lines.len().saturating_sub(1));
            for i in start_idx..=end_idx {
                let line_str = lines[i];
                if let Some(pos) = line_str.find(clean_name) {
                    let line_start = text.lines().take(i).map(|l| l.len() + 1).sum::<usize>();
                    let abs_offset = line_start + pos;
                    if declared_type_span_polyglot(text, abs_offset, lang).is_some() {
                        return Some(abs_offset);
                    }
                }
            }
        }
    }

    for (idx, _) in text.match_indices(clean_name) {
        if idx > 0
            && text[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let after = &text[idx + clean_name.len()..];
        if after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        if crate::inline_parameter::is_in_comment(text, idx, lang) {
            continue;
        }
        if is_import_line(text, idx, lang) {
            continue;
        }
        if declared_type_span_polyglot(text, idx, lang).is_some() {
            return Some(idx);
        }
    }

    None
}
