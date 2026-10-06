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

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `word` occurs in `text` as a whole identifier.
pub(crate) fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(is_ident)
            && !text[at + word.len()..].chars().next().is_some_and(is_ident)
    })
}

/// The type an `impl` header is for: `impl<T> Trait for Wrapper<T> where …` gives `Wrapper`.
pub fn self_type(header: &str) -> Option<String> {
    let mut rest = header.trim_start();
    if let Some(generics) = rest.strip_prefix('<') {
        let mut depth = 1i32;
        let mut end = generics.len();
        for (i, c) in generics.char_indices() {
            match c {
                '<' => depth += 1,
                '>' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        rest = &generics[end..];
    }
    let rest = rest.split(" where").next().unwrap_or(rest);
    let ty = match rest.find(" for ") {
        Some(at) => &rest[at + " for ".len()..],
        None => rest,
    };
    let ty = ty.trim().trim_start_matches('&').trim_start_matches("mut ");
    let path = ty.split('<').next().unwrap_or(ty).trim();
    let name = path.rsplit("::").next().unwrap_or(path).trim();
    (!name.is_empty() && name.chars().all(is_ident)).then(|| name.to_string())
}

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn source_line(text: &str, at: usize) -> String {
    let start = text[..at].rfind('\n').map_or(0, |i| i + 1);
    text[start..]
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string()
}

pub(crate) fn is_in_literal_or_comment(text: &str, at: usize, lang: Language) -> bool {
    let mut chars = text[..at].chars().peekable();
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
