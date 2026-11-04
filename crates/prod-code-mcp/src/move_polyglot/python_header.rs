/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::imports::py_from_module;
use super::specifiers::is_symbol_used;
use crate::caller_migration::lexical_code_mask;
use crate::parameter_object::Language;

pub(crate) fn remove_from_py_from_import(line: &str, sym: &str) -> (String, bool) {
    let Some(pos) = line.find(" import ") else {
        return (line.to_string(), false);
    };
    let before = &line[..pos + 8];
    let after = &line[pos + 8..];
    let items: Vec<&str> = after
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    let mut new_items = Vec::new();
    let mut removed = false;
    for item in items {
        let first_word = item.split_whitespace().next().unwrap_or("");
        if first_word == sym {
            removed = true;
        } else {
            new_items.push(item);
        }
    }
    if !removed {
        return (line.to_string(), false);
    }
    if new_items.is_empty() {
        return (String::new(), true);
    }
    (format!("{before}{}", new_items.join(", ")), true)
}

pub fn insert_or_merge_py_import(content: &str, sym: &str, mod_spec: &str) -> (String, bool) {
    let mask = lexical_code_mask(content, Language::Python);
    let mut offset = 0;
    for chunk in content.split_inclusive('\n') {
        let line = chunk.strip_suffix('\n').unwrap_or(chunk);
        let trimmed = line.trim();
        if mask.get(offset) == Some(&true)
            && py_from_module(trimmed) == Some(mod_spec)
            && trimmed.contains(" import ")
            && !trimmed
                .chars()
                .any(|c| matches!(c, '(' | ')' | '#' | ';' | '\\' | '*'))
        {
            if is_symbol_used(line, sym) {
                return (content.to_string(), false);
            }
            let mut replaced = content.to_string();
            replaced.replace_range(offset..offset + line.len(), &format!("{line}, {sym}"));
            return (replaced, true);
        }
        offset += chunk.len();
    }
    let import_line = format!("from {mod_spec} import {sym}\n");
    let insert_pos = after_future_imports(content)
        .or_else(|| after_module_docstring(content))
        .unwrap_or_else(|| preamble_end(content));
    let mut out = String::with_capacity(content.len() + import_line.len());
    out.push_str(&content[..insert_pos]);
    if insert_pos > 0
        && !content[..insert_pos].ends_with('\n')
        && content.as_bytes().get(insert_pos - 1) == Some(&b';')
    {
        out.push(' ');
        out.push_str(import_line.trim_end());
        out.push(';');
        out.push_str(&content[insert_pos..]);
        return (out, true);
    }
    if insert_pos > 0 && !content[..insert_pos].ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&import_line);
    out.push_str(&content[insert_pos..]);
    (out, true)
}

pub(super) fn preamble_end(content: &str) -> usize {
    let mut end = 0;
    for line in content.split_inclusive('\n') {
        let text = line.trim();
        if !text.is_empty() && !text.starts_with('#') {
            break;
        }
        end += line.len();
    }
    end
}

fn string_end(bytes: &[u8], mut at: usize) -> Option<usize> {
    if matches!(bytes.get(at), Some(b'r' | b'R' | b'u' | b'U')) {
        at += 1;
    }
    let quote = *bytes.get(at)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    let width = if bytes.get(at..at + 3) == Some(&[quote, quote, quote]) {
        3
    } else {
        1
    };
    at += width;
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            at = (at + 2).min(bytes.len());
        } else if bytes
            .get(at..at + width)
            .is_some_and(|s| s.iter().all(|b| *b == quote))
        {
            return Some(at + width);
        } else {
            at += 1;
        }
    }
    None
}

fn parenthesized_docstring(content: &str, start: usize) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut at = start;
    let mut depth = 0usize;
    let mut saw_string = false;
    while at < bytes.len() {
        match bytes[at] {
            b'(' => {
                if saw_string && depth > 0 {
                    return None;
                }
                depth += 1;
                at += 1;
            }
            b')' => {
                depth = depth.checked_sub(1)?;
                at += 1;
                if depth == 0 {
                    let mut next = at;
                    while matches!(bytes.get(next), Some(b' ' | b'\t')) {
                        next += 1;
                    }
                    if saw_string && bytes.get(next) == Some(&b';') {
                        return Some(next + 1);
                    }
                    let end = content[at..]
                        .find('\n')
                        .map_or(content.len(), |n| at + n + 1);
                    let tail = content[at..end].trim();
                    return (saw_string && (tail.is_empty() || tail.starts_with('#')))
                        .then_some(end);
                }
            }
            b'#' => {
                at += content[at..]
                    .find('\n')
                    .map_or(content.len() - at, |n| n + 1)
            }
            b if b.is_ascii_whitespace() => at += 1,
            _ => {
                if matches!(bytes.get(at), Some(b'r' | b'R' | b'u' | b'U')) {
                    at += 1;
                }
                let quote = *bytes.get(at)?;
                if quote != b'\'' && quote != b'"' {
                    return None;
                }
                let width = if bytes.get(at..at + 3) == Some(&[quote, quote, quote]) {
                    3
                } else {
                    1
                };
                at += width;
                let mut closed = false;
                while at < bytes.len() {
                    if bytes[at] == b'\\' {
                        at = (at + 2).min(bytes.len());
                    } else if bytes
                        .get(at..at + width)
                        .is_some_and(|s| s.iter().all(|b| *b == quote))
                    {
                        at += width;
                        closed = true;
                        break;
                    } else {
                        at += 1;
                    }
                }
                if !closed {
                    return None;
                }
                saw_string = true;
            }
        }
    }
    None
}

/// Locate a plain leading module docstring, including multiline quoted strings.
pub(super) fn after_module_docstring(content: &str) -> Option<usize> {
    let bytes = content.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at].is_ascii_whitespace() {
            at += 1;
        } else if bytes[at] == b'#' {
            at += content[at..]
                .find('\n')
                .map_or(content.len() - at, |n| n + 1);
        } else {
            break;
        }
    }
    if bytes.get(at) == Some(&b'(') {
        return parenthesized_docstring(content, at);
    }
    at = string_end(bytes, at)?;
    loop {
        while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\r')) {
            at += 1;
        }
        match bytes.get(at) {
            None => return Some(at),
            Some(b'\n') => return Some(at + 1),
            Some(b';') => return Some(at + 1),
            Some(b'#') => {
                return Some(
                    content[at..]
                        .find('\n')
                        .map_or(content.len(), |n| at + n + 1),
                );
            }
            Some(b'\\') if bytes.get(at + 1) == Some(&b'\n') => {
                at += 2;
            }
            _ => at = string_end(bytes, at)?,
        }
    }
}

fn is_future_statement(statement: &str) -> bool {
    let mut words = statement.split_whitespace();
    words.next() == Some("from")
        && words.next() == Some("__future__")
        && words
            .next()
            .is_some_and(|word| word == "import" || word.starts_with("import("))
}

/// End of the last top-level future import, excluding text inside strings and comments.
pub(super) fn after_future_imports(content: &str) -> Option<usize> {
    let mask = lexical_code_mask(content, Language::Python);
    let bytes = content.as_bytes();
    let mut offset = 0;
    let mut depth = 0usize;
    let mut continuation = false;
    let mut statement = String::new();
    let mut result = None;
    for line in content.split_inclusive('\n') {
        let end = offset + line.len();
        let code: Vec<u8> = (offset..end)
            .map(|i| if mask[i] { bytes[i] } else { b' ' })
            .collect();
        let start = if code.starts_with(b"from") {
            Some(0)
        } else {
            code.iter()
                .enumerate()
                .filter(|(_, b)| **b == b';')
                .find_map(|(i, _)| {
                    let tail = &code[i + 1..];
                    let skip = tail.iter().position(|b| !b.is_ascii_whitespace())?;
                    tail[skip..].starts_with(b"from").then_some(i + 1 + skip)
                })
        };
        if start.is_some() || continuation {
            let start = if continuation { 0 } else { start.unwrap_or(0) };
            statement.push_str(&String::from_utf8_lossy(&code[start..]).replace('\\', " "));
            for byte in &code {
                match byte {
                    b'(' => depth += 1,
                    b')' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
            let last = code.iter().rev().find(|b| !b.is_ascii_whitespace());
            continuation = depth != 0 || last == Some(&b'\\');
            if !continuation {
                if is_future_statement(&statement) {
                    let mut position = end;
                    for (index, _) in code
                        .iter()
                        .enumerate()
                        .filter(|(i, b)| *i >= start && **b == b';')
                    {
                        let rest = String::from_utf8_lossy(&code[index + 1..]);
                        let next = rest.trim_start();
                        if !next.is_empty() && !is_future_statement(next) {
                            position = offset + index + 1;
                            break;
                        }
                    }
                    result = Some(position);
                }
                statement.clear();
            }
        }
        offset = end;
    }
    result
}
