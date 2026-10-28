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

fn has_top_level_assignment(text: &str, lang: Language) -> bool {
    let mut paren_depth = 0usize;
    let mut bracket_depth = 0usize;
    let mut brace_depth = 0usize;
    for (at, character) in text.char_indices() {
        if is_in_literal_or_comment(text, at, lang) {
            continue;
        }
        match character {
            '(' => paren_depth += 1,
            ')' => paren_depth = paren_depth.saturating_sub(1),
            '[' => bracket_depth += 1,
            ']' => bracket_depth = bracket_depth.saturating_sub(1),
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            '=' if paren_depth == 0 && bracket_depth == 0 && brace_depth == 0 => return true,
            _ => {}
        }
    }
    false
}

pub(crate) fn javascript_class_member_header_start(text: &str, lang: Language) -> usize {
    let member_start = text
        .char_indices()
        .rev()
        .find(|(at, character)| {
            matches!(character, ';' | '}') && !is_in_literal_or_comment(text, *at, lang)
        })
        .map_or(0, |(at, _)| at + 1);
    let member_text = &text[member_start..];
    if !has_top_level_assignment(member_text, lang) {
        return member_start;
    }
    member_text
        .char_indices()
        .rev()
        .find(|(at, character)| {
            *character == '\n' && !is_in_literal_or_comment(member_text, *at, lang)
        })
        .map_or(member_start, |(at, _)| member_start + at + 1)
}

fn javascript_regex_end(text: &str, start: usize) -> Option<usize> {
    let mut at = start + 1;
    let mut in_character_class = false;
    let mut escaped = false;
    while at < text.len() {
        let character = text[at..].chars().next()?;
        if matches!(character, '\n' | '\r') {
            return None;
        }
        let next = at + character.len_utf8();
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '[' {
            in_character_class = true;
        } else if character == ']' {
            in_character_class = false;
        } else if character == '/' && !in_character_class {
            let mut end = next;
            while text[end..]
                .chars()
                .next()
                .is_some_and(|flag| flag.is_ascii_alphabetic())
            {
                end += text[end..].chars().next()?.len_utf8();
            }
            return Some(end);
        }
        at = next;
    }
    None
}

fn javascript_literal_or_comment_contains(text: &str, target: usize) -> bool {
    let mut at = 0usize;
    let mut expression_can_start = true;
    while at < text.len() && at <= target {
        let character = text[at..].chars().next().unwrap();
        if character.is_whitespace() {
            at += character.len_utf8();
            continue;
        }
        if character == '/' {
            let next = text[at + 1..].chars().next();
            if next == Some('/') {
                let start = at;
                at += 2;
                while at < text.len() {
                    let current = text[at..].chars().next().unwrap();
                    if matches!(current, '\n' | '\r') {
                        break;
                    }
                    at += current.len_utf8();
                }
                if (start..at).contains(&target) {
                    return true;
                }
                continue;
            }
            if next == Some('*') {
                let start = at;
                at += 2;
                while at < text.len() && !text[at..].starts_with("*/") {
                    at += text[at..].chars().next().unwrap().len_utf8();
                }
                at = (at + 2).min(text.len());
                if (start..at).contains(&target) {
                    return true;
                }
                continue;
            }
            if expression_can_start && let Some(end) = javascript_regex_end(text, at) {
                if (at..end).contains(&target) {
                    return true;
                }
                at = end;
                expression_can_start = false;
                continue;
            }
            if expression_can_start {
                return true;
            }
            at += character.len_utf8();
            expression_can_start = true;
            continue;
        }
        if matches!(character, '\'' | '"' | '`') {
            let start = at;
            let delimiter = character;
            at += character.len_utf8();
            let mut escaped = false;
            while at < text.len() {
                let current = text[at..].chars().next().unwrap();
                at += current.len_utf8();
                if escaped {
                    escaped = false;
                } else if current == '\\' {
                    escaped = true;
                } else if current == delimiter {
                    break;
                }
            }
            if (start..at).contains(&target) {
                return true;
            }
            expression_can_start = false;
            continue;
        }
        if is_ident(character) {
            let start = at;
            at += character.len_utf8();
            while at < text.len() {
                let next = text[at..].chars().next().unwrap();
                if !is_ident(next) {
                    break;
                }
                at += next.len_utf8();
            }
            expression_can_start = matches!(
                &text[start..at],
                "return"
                    | "throw"
                    | "case"
                    | "delete"
                    | "void"
                    | "typeof"
                    | "new"
                    | "in"
                    | "instanceof"
                    | "yield"
                    | "await"
                    | "else"
                    | "do"
                    | "of"
                    | "if"
                    | "while"
                    | "for"
                    | "switch"
                    | "catch"
                    | "with"
            );
            continue;
        }
        if character.is_ascii_digit() {
            at += character.len_utf8();
            while at < text.len() {
                let next = text[at..].chars().next().unwrap();
                if !(next.is_ascii_alphanumeric() || matches!(next, '.' | '_')) {
                    break;
                }
                at += next.len_utf8();
            }
            expression_can_start = false;
            continue;
        }
        if matches!(character, '+' | '-') && text[at + 1..].starts_with(character) {
            let operand_can_start = expression_can_start;
            at += character.len_utf8() * 2;
            expression_can_start = operand_can_start;
            continue;
        }
        expression_can_start = !matches!(character, ')' | ']' | '}' | '.');
        at += character.len_utf8();
    }
    false
}

pub(crate) fn is_in_literal_or_comment(text: &str, at: usize, lang: Language) -> bool {
    if matches!(lang, Language::JavaScript | Language::TypeScript) {
        return javascript_literal_or_comment_contains(text, at);
    }
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
