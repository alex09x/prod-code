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
use anyhow::{Result, ensure};

/// Indentation of a Python simple statement comes from the start of its
/// logical line, even after semicolon continuations across brackets,
/// multiline strings, or backslash line joins.
pub(crate) fn logical_indent(text: &str, start: usize) -> String {
    let raw = crate::caller_migration::lexical_code_mask(text, Language::Python);
    let (mask, _) = statement_mask(text, &raw);
    let (line, _) = logical_lines(text, start, &mask);
    let slice = if line == 0 {
        text[line..start]
            .strip_prefix('\u{feff}')
            .unwrap_or(&text[line..start])
    } else {
        &text[line..start]
    };
    slice
        .chars()
        .take_while(|c| matches!(c, ' ' | '\t' | '\x0c'))
        .collect()
}

/// Find the beginning of the current logical line, including continued headers.
/// An inline suite needs structural expansion; leave it untouched for now rather
/// than dedenting a staged setter into the surrounding scope. Ordinary semicolon
/// statements, including lambda/dict expressions, remain supported.
pub(super) fn inline_suite(text: &str, start: usize, mask: &[bool]) -> bool {
    // Interpolation code is executable, but its newlines and delimiters still
    // belong to the enclosing string expression, not the surrounding statement.
    let (mask, _) = statement_mask(text, mask);
    let (line, lines) = logical_lines(text, start, &mask);
    let prefix = header_start(&text[line..start], line == 0);
    let prefix_start = start - prefix.len();
    // Python identifiers use XID_Continue, including combining marks and
    // connector punctuation. Match keywords only after the complete token.
    let word = prefix
        .split(|ch| !unicode_ident::is_xid_continue(ch))
        .next()
        .unwrap_or("");
    matches!(
        word,
        "if" | "elif"
            | "else"
            | "while"
            | "for"
            | "try"
            | "except"
            | "finally"
            | "with"
            | "def"
            | "class"
            | "async"
    ) || (word == "case"
        && in_match_suite(&lines, &text[line..start])
        && !prefix[word.len()..].trim_start().starts_with(['=', ':'])
        && case_suite_colon(text, prefix_start + word.len(), start, &mask))
}

fn logical_lines<'a>(text: &'a str, start: usize, mask: &[bool]) -> (usize, Vec<&'a str>) {
    let mut depth = 0usize;
    let mut line = 0;
    let mut lines = Vec::new();
    for (at, ch) in text[..start].char_indices() {
        if !mask[at] {
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '\n' if depth == 0 && !line_continued(text, at, mask) => {
                lines.push(&text[line..at]);
                line = at + 1;
            }
            _ => {}
        }
    }
    (line, lines)
}

/// Only executable backslashes join physical lines. A comment ending in '\\'
/// still ends here, with either LF or CRLF.
fn line_continued(text: &str, newline_at: usize, mask: &[bool]) -> bool {
    text[..newline_at]
        .trim_end_matches('\r')
        .len()
        .checked_sub(1)
        .is_some_and(|last| mask[last] && text.as_bytes()[last] == b'\\')
}

/// Hide complete string expressions for statement-boundary inspection only.
/// The shared executable mask must keep exposing field references in f-strings.
fn statement_mask(text: &str, executable: &[bool]) -> (Vec<bool>, Vec<bool>) {
    let mut mask = executable.to_vec();
    // A comment may immediately follow an opaque quote. Record its identity
    // while skipping whole strings, independently of executable transitions.
    let mut comments = vec![false; text.len()];
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'#' if !executable[at] => {
                comments[at] = true;
                at = text[at..].find('\n').map_or(bytes.len(), |n| at + n);
            }
            b'\'' | b'"' if !executable[at] => {
                let end = string_end(bytes, at);
                mask[at..end].fill(false);
                at = end;
            }
            _ => at += 1,
        }
    }
    (mask, comments)
}

/// Skip nested strings and replacement fields as a unit, including format
/// fields. Quote reuse inside a replacement must not close its outer string.
fn string_end(bytes: &[u8], at: usize) -> usize {
    let quote = bytes[at];
    let width = if bytes[at..].starts_with(&[quote; 3]) {
        3
    } else {
        1
    };
    let prefix = bytes[..at]
        .iter()
        .rev()
        .take_while(|b| b.is_ascii_alphabetic());
    let formatted = prefix.clone().count() <= 2 && prefix.clone().any(|b| matches!(b, b'f' | b'F'));
    let mut end = at + width;
    while end < bytes.len() {
        if bytes[end..].starts_with(&bytes[at..at + width]) {
            return end + width;
        }
        match bytes[end] {
            b'\\' => end = (end + 2).min(bytes.len()),
            b'{' if formatted && bytes.get(end + 1) == Some(&b'{') => end += 2,
            b'{' if formatted => end = replacement_end(bytes, end + 1),
            _ => end += 1,
        }
    }
    end
}

fn replacement_end(bytes: &[u8], mut at: usize) -> usize {
    let mut depth = 1;
    let mut brackets = 0usize;
    while at < bytes.len() {
        match bytes[at] {
            b'\'' | b'"' => {
                at = string_end(bytes, at);
                continue;
            }
            b'#' => {
                while at < bytes.len() && bytes[at] != b'\n' {
                    at += 1;
                }
                continue;
            }
            b'{' => depth += 1,
            b'(' | b'[' => brackets += 1,
            b')' | b']' => brackets = brackets.saturating_sub(1),
            b':' if depth == 1 && brackets == 0 => return format_end(bytes, at + 1),
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return at + 1;
                }
            }
            _ => {}
        }
        at += 1;
    }
    at
}

/// Format text can contain quotes and comment markers literally; only nested
/// replacement fields re-enter expression syntax.
fn format_end(bytes: &[u8], mut at: usize) -> usize {
    while at < bytes.len() {
        match bytes[at] {
            b'{' => at = replacement_end(bytes, at + 1),
            b'}' => return at + 1,
            _ => at += 1,
        }
    }
    at
}

/// Explicit line joins can precede the first token of a logical line. Skip
/// them along with indentation without changing offsets in the original text.
/// A comment or string starts with its own marker, so its backslashes stay put.
/// Only the absolute file start may contain Python's UTF-8 signature.
fn header_start(mut text: &str, file_start: bool) -> &str {
    if file_start {
        text = text.strip_prefix('\u{feff}').unwrap_or(text);
    }
    loop {
        text = text.trim_start();
        match text
            .strip_prefix("\\\r\n")
            .or_else(|| text.strip_prefix("\\\n"))
        {
            Some(rest) => text = rest,
            None => return text,
        }
    }
}

/// The soft keyword belongs to a pattern header only directly inside a match
/// suite. In other suites `case.value: T` and `case[index]: T` are annotations.
/// Whitespace alone cannot distinguish them: `case[0]:` is also a valid pattern.
/// Logical lines keep continued match headers and their arbitrary indentation
/// together; blank lines and comments cannot change the enclosing suite.
fn in_match_suite(lines: &[&str], current: &str) -> bool {
    let indent = indentation(current);
    lines
        .iter()
        .enumerate()
        .rev()
        .map(|(index, line)| (*line, header_start(line, index == 0)))
        .find(|(line, code)| {
            !code.is_empty() && !code.starts_with('#') && indentation(line) < indent
        })
        .is_some_and(|(_, code)| {
            code.strip_prefix("match")
                .is_some_and(|rest| rest.starts_with(|ch| !unicode_ident::is_xid_continue(ch)))
        })
}

fn indentation(line: &str) -> usize {
    line.chars()
        .take_while(|ch| matches!(ch, ' ' | '\t' | '\x0c'))
        .fold(0, |col, ch| match ch {
            '\t' => (col / 8 + 1) * 8,
            '\x0c' => 0,
            _ => col + 1,
        })
}

/// `case` is also an ordinary identifier. A pattern suite has an executable
/// colon outside all pattern delimiters, before the first simple-statement
/// separator. The enclosing match suite and leading-token checks distinguish
/// patterns from ordinary identifiers. A lambda default in a pattern guard can
/// contain `=` before its colon; that colon also suffices to refuse expansion.
/// Colons in calls, subscripts and strings do not count.
fn case_suite_colon(text: &str, from: usize, to: usize, mask: &[bool]) -> bool {
    let mut depth = 0usize;
    for (off, ch) in text[from..to].char_indices() {
        let at = from + off;
        if !mask[at] {
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ':' if depth == 0 => return true,
            ';' if depth == 0 => return false,
            _ => {}
        }
    }
    false
}

/// Python assigns an entire expression-list, including unparenthesized tuples.
/// Lambda parameter commas/defaults and body expressions belong to that list;
/// only a logical newline, comment or semicolon ends a standalone assignment.
pub(super) fn end(text: &str, start: usize) -> Result<usize> {
    let raw = crate::caller_migration::lexical_code_mask(text, Language::Python);
    let (mask, comments) = statement_mask(text, &raw);
    let mut brackets = Vec::new();
    let mut parameters = Vec::new();
    let mut end = text.len();
    for (off, ch) in text[start..].char_indices() {
        let at = start + off;
        if !mask[at] {
            if comments[at] && brackets.is_empty() {
                end = text[..at].trim_end().len();
                break;
            }
            continue;
        }
        if brackets.is_empty() && (ch == ';' || (ch == '\n' && !line_continued(text, at, &mask))) {
            end = at;
            break;
        }
        match ch {
            '(' | '[' | '{' => brackets.push(ch),
            ')' | ']' | '}' => {
                let expected = match ch {
                    ')' => '(',
                    ']' => '[',
                    _ => '{',
                };
                ensure!(
                    brackets.pop() == Some(expected),
                    "cannot prove Python RHS delimiter boundary; nothing was written"
                );
            }
            'l' if text[at..].starts_with("lambda")
                && (at == 0
                    || !super::super::case::is_ident(text[..at].chars().next_back().unwrap()))
                && text[at + 6..]
                    .chars()
                    .next()
                    .is_none_or(|ch| !super::super::case::is_ident(ch)) =>
            {
                parameters.push(brackets.len());
            }
            ':' if parameters.last() == Some(&brackets.len()) => {
                parameters.pop();
            }
            '=' if brackets.is_empty() && parameters.is_empty() => {
                let previous = text[..at].chars().next_back();
                let comparison = previous.is_some_and(|ch| "=!<>:".contains(ch))
                    || text[at + 1..].starts_with('=');
                ensure!(
                    comparison,
                    "chained Python assignment cannot become a setter argument; nothing was written"
                );
            }
            _ => {}
        }
    }
    ensure!(
        brackets.is_empty() && parameters.is_empty(),
        "cannot prove complete Python RHS; nothing was written"
    );
    Ok(end)
}
