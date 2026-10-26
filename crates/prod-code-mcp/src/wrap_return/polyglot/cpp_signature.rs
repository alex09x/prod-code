/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::parameter_object::{self, Language};

/// A fallback spelling key, not a C++ type resolver. Keep token boundaries and
/// literal contents; exact semantic references take precedence over this key.
pub(super) fn tokens(text: &str) -> Vec<String> {
    token_spans(text)
        .into_iter()
        .map(|(start, end)| text[start..end].to_string())
        .collect()
}

/// Preserve literal tokens atomically when editing a C++ type. Comments have no
/// token span and can therefore remain byte-for-byte in their original place.
pub(super) fn token_spans(text: &str) -> Vec<(usize, usize)> {
    static TOKEN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r#"(?s:(//[^\n]*|/\*.*?\*/))|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|[\p{L}_][\p{L}\p{N}_]*|[0-9][\w.]*|::|&&|\|\||<<|>>|<=|>=|==|!=|->|\+\+|--|\.\.\.|[^\s]"#).unwrap()
    });
    let mut spans = Vec::new();
    let mut consumed = 0;
    while consumed < text.len() {
        let Some(capture) = TOKEN.captures(&text[consumed..]) else {
            break;
        };
        let base = consumed;
        let token = capture.get(0).unwrap();
        let start = base + token.start();
        let mut end = base + token.end();
        consumed = end;
        if capture.get(1).is_some() {
            continue;
        }
        if matches!(token.as_str(), "R" | "u8R" | "uR" | "UR" | "LR")
            && let Some(tail) = text[end..].strip_prefix('"')
            && let Some(open) = tail.find('(').filter(|&at| at <= 16)
        {
            let delimiter = &tail[..open];
            if !delimiter
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, ')' | '\\'))
                && let Some(close) = tail[open + 1..].find(&format!("){delimiter}\""))
            {
                end += 1 + open + 1 + close + delimiter.len() + 2;
            }
        }
        consumed = end;
        spans.push((start, end));
    }
    spans
}

/// Declaration attributes are not parameter bindings or function-type tokens.
/// Keep their arguments opaque, including any brackets inside string tokens.
fn parameter_tokens(text: &str) -> Vec<String> {
    let words = tokens(text);
    let mut result = Vec::new();
    let mut i = 0;
    while i < words.len() {
        if words[i] == "[" && words.get(i + 1).is_some_and(|t| t == "[") {
            let mut depth = 2usize;
            i += 2;
            while i < words.len() && depth > 0 {
                match words[i].as_str() {
                    "[" => depth += 1,
                    "]" => depth -= 1,
                    _ => {}
                }
                i += 1;
            }
            if depth > 0 {
                return words;
            }
        } else {
            result.push(words[i].clone());
            i += 1;
        }
    }
    result
}

pub(super) fn parameters(text: &str, at: usize, name: &str) -> Option<(usize, Vec<Vec<String>>)> {
    let mask = crate::caller_migration::mask::lexical_code_mask(text, Language::Cpp);
    let open = (at + name.len()..text.len())
        .find(|&i| mask[i] && !text.as_bytes()[i].is_ascii_whitespace())?;
    if text.as_bytes().get(open) != Some(&b'(') {
        return None;
    }
    let close = parameter_object::matching_bracket(text, open)?;
    let list = &text[open + 1..close];
    if tokens(list) == ["void"] {
        return Some((close, Vec::new()));
    }
    let mut types = Vec::new();
    for (_, entry) in parameter_object::entries(list, Language::Cpp) {
        let (declaration, _) = parameter_object::split_default(entry, Language::Cpp);
        // Reuse the existing declarator facility to locate a parameter name.
        // Multiword unnamed types end in a keyword; cv-qualified unnamed types
        // have no type before the reported name. Neither is a binding.
        let param = parameter_object::params::parse_param(declaration, 0, Language::Cpp);
        let prefix = &declaration[..param.name_at];
        let has_type = parameter_tokens(prefix).iter().any(|t| {
            !matches!(
                t.as_str(),
                "const" | "volatile" | "struct" | "class" | "union" | "enum" | "typename"
            )
        });
        let keyword = matches!(
            param.name.as_str(),
            "void"
                | "bool"
                | "char"
                | "short"
                | "int"
                | "long"
                | "float"
                | "double"
                | "signed"
                | "unsigned"
                | "const"
                | "volatile"
                | "wchar_t"
                | "char8_t"
                | "char16_t"
                | "char32_t"
                | "auto"
                | "__int128"
                | "__int64"
                | "__int32"
                | "__int16"
                | "__int8"
                | "_Float128"
                | "_Float64"
                | "_Float32"
                | "_Complex"
                | "__complex__"
        );
        let spelling = if !param.name.is_empty() && !keyword && has_type {
            format!(
                "{} {}",
                prefix,
                &declaration[param.name_at + param.name.len()..]
            )
        } else {
            declaration.to_string()
        };
        types.push(parameter_tokens(&spelling));
    }
    Some((close, types))
}

pub(super) fn include_guard(text: &str) -> Option<String> {
    let mask = crate::caller_migration::mask::lexical_code_mask(text, Language::Cpp);
    let clean = String::from_utf8(
        text.bytes()
            .zip(&mask)
            .map(|(b, yes)| if *yes || b == b'\n' { b } else { b' ' })
            .collect(),
    )
    .expect("mask preserves UTF-8 code");
    let lines: Vec<_> = clean
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let directive =
        |line: &str| -> Vec<String> { line.strip_prefix('#').map(tokens).unwrap_or_default() };
    let first = lines.first().map(|l| directive(l)).unwrap_or_default();
    let second = lines.get(1).map(|l| directive(l)).unwrap_or_default();
    (first.len() == 2
        && first[0] == "ifndef"
        && second == ["define".to_string(), first[1].clone()]
        && lines.last().is_some_and(|l| directive(l) == ["endif"]))
    .then(|| first[1].clone())
}

/// Only conditionals enclosing this declaration affect fallback identity. A
/// conventional outer include guard is transparent; other branches need semantic
/// identity. Closed conditionals elsewhere in the header do not taint it.
pub(super) fn conditional(text: &str, at: usize) -> bool {
    let mask = crate::caller_migration::mask::lexical_code_mask(text, Language::Cpp);
    let clean = String::from_utf8(
        text.bytes()
            .zip(&mask)
            .map(|(b, yes)| if *yes || b == b'\n' { b } else { b' ' })
            .collect(),
    )
    .expect("mask preserves UTF-8 code");
    let directive =
        |line: &str| -> Vec<String> { line.strip_prefix('#').map(tokens).unwrap_or_default() };
    let guard = include_guard(text);
    let mut offset = 0;
    let mut stack = Vec::new();
    let mut first_if = true;
    for line in clean.split_inclusive('\n') {
        if offset >= at {
            break;
        }
        offset += line.len();
        let words = directive(line.trim());
        match words.first().map(String::as_str) {
            Some("if" | "ifdef" | "ifndef") => {
                stack.push(
                    guard
                        .as_ref()
                        .is_some_and(|name| first_if && words == ["ifndef", name.as_str()]),
                );
                first_if = false;
            }
            Some("else" | "elif") => {
                if let Some(top) = stack.last_mut() {
                    *top = false;
                }
            }
            Some("endif") => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.contains(&false)
}
