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

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn is_in_string_or_comment(content: &str, at: usize, lang: Language) -> bool {
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

pub fn one_based_lsp_position(text: &str, byte_offset: usize) -> (u32, u32) {
    let before = &text[..byte_offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column_text = before.rsplit('\n').next().unwrap_or_default();
    let character = column_text.encode_utf16().count() as u32 + 1;
    (line, character)
}

pub fn is_import_export_call_context(text: &str, at: usize, lang: Language) -> bool {
    if !matches!(lang, Language::TypeScript | Language::JavaScript) {
        return crate::inline_parameter::is_import_or_export_context(text, at, lang);
    }
    let line_start = text[..at].rfind('\n').map_or(0, |position| position + 1);
    let line_end = text[at..]
        .find('\n')
        .map_or(text.len(), |offset| at + offset);
    let line = text[line_start..line_end].trim_start();
    if line.starts_with("import ")
        || line.starts_with("import{")
        || line.starts_with("export {")
        || line.starts_with("export{")
        || line.starts_with("export *")
        || line.starts_with("from ")
        || line.contains("require(")
    {
        return true;
    }
    let before = &text[..at];
    before
        .rfind("import {")
        .or_else(|| before.rfind("export {"))
        .is_some_and(|start| !before[start..].contains('}'))
}

pub fn nested_method_header(header: &str) -> bool {
    let header = header.rsplit('{').next().unwrap_or(header).trim();
    let Some(open) = header.rfind('(') else {
        return false;
    };
    let name = header[..open]
        .split_whitespace()
        .next_back()
        .unwrap_or_default()
        .trim_start_matches('*')
        .trim_start_matches('&');
    if matches!(name, "if" | "for" | "while" | "switch" | "catch" | "with") {
        return false;
    }
    let rest = header[open..].trim_end();
    rest.ends_with(')')
        || [
            " const",
            " async",
            " throws",
            " rethrows",
            " noexcept",
            " override",
            " final",
        ]
        .iter()
        .any(|suffix| rest.ends_with(suffix))
}

/// The `return` keywords that return from the function whose body is `inner`: not one inside a
/// closure or an `async` block, where `return` returns from that instead.
pub fn own_returns(inner: &str) -> Vec<usize> {
    let bytes = inner.as_bytes();
    let mut out = Vec::new();
    let mut stack: Vec<bool> = Vec::new(); // true: a closure or async block
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        if inner[i..].starts_with("//") {
            i = inner[i..].find('\n').map_or(bytes.len(), |n| i + n);
            continue;
        }
        if inner[i..].starts_with("/*") {
            i += 2;
            let mut depth = 1usize;
            while i < bytes.len() && depth > 0 {
                if inner[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if inner[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += inner[i..].chars().next().unwrap().len_utf8();
                }
            }
            continue;
        }
        if matches!(c, b'"' | b'\'' | b'`') {
            let quote = c;
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 1;
                    if i < bytes.len() {
                        i += inner[i..].chars().next().unwrap().len_utf8();
                    }
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += inner[i..].chars().next().unwrap().len_utf8();
                }
            }
            continue;
        }
        if c == b'{' {
            let head = inner[..i].trim_end();
            let line = &head[head.rfind('\n').map_or(0, |n| n + 1)..];
            // A closure, an `async` block or a nested `fn` has returns of its own.
            let opaque = head.ends_with("async")
                || head.ends_with("async move")
                || head.ends_with('|')
                || (line.contains('|') && line.contains("->"))
                || line.trim_start().starts_with("fn ")
                || line.contains(" fn ")
                || line.contains("func ")
                || line.contains("function ")
                || line.contains("=>")
                || nested_method_header(line);
            stack.push(opaque);
            i += 1;
            continue;
        }
        if c == b'}' {
            stack.pop();
            i += 1;
            continue;
        }
        if inner[i..].starts_with("return")
            && !inner[..i].chars().next_back().is_some_and(is_ident)
            && !inner[i + 6..].chars().next().is_some_and(is_ident)
            && !stack.iter().any(|opaque| *opaque)
            && !inner[..i].trim_end().ends_with('|')
        {
            out.push(i);
        }
        i += inner[i..].chars().next().unwrap().len_utf8();
    }
    out
}

/// Where the expression after `return` at `at` ends: its `;`, or the end of the text.
pub fn return_value_end(inner: &str, at: usize) -> usize {
    let bytes = inner.as_bytes();
    let mut i = at + "return".len();
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => match crate::parameter_object::matching_bracket(inner, i) {
                Some(close) => i = close + 1,
                None => return bytes.len(),
            },
            b';' | b'}' | b'\n' => return i,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// The body of the inverted function, from the body of the original (the text between its braces).
pub fn negated_body(inner: &str) -> String {
    let mut body = inner.to_string();
    for at in own_returns(inner).into_iter().rev() {
        let end = return_value_end(inner, at);
        let value = inner[at + "return".len()..end].trim();
        body.replace_range(at..end, &format!("return !({value})"));
    }
    let trimmed = body.trim();
    // A body that is one expression reads best negated in place; anything with statements is
    // negated as a block, whose value is its tail.
    if !trimmed.contains(';') && !trimmed.contains("return") && !trimmed.is_empty() {
        return format!("\n    !({trimmed})\n");
    }
    let indented: String = body
        .trim_matches('\n')
        .lines()
        .map(|l| {
            if l.trim().is_empty() {
                String::new()
            } else {
                format!("    {l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("\n    !{{\n{indented}\n    }}\n")
}

/// Where a call whose callee name starts at `at` begins: the start of its receiver chain for a
/// method call, of its path for a path call, or the name itself.
pub fn call_start(text: &str, at: usize) -> usize {
    let before = text[..at].trim_end();
    if let Some(dot_end) = before.strip_suffix('.').map(|b| b.len()) {
        return crate::encapsulate_field::chain_start(text, dot_end);
    }
    if let Some(arrow_end) = before.strip_suffix("->").map(|b| b.len()) {
        return crate::encapsulate_field::chain_start(text, arrow_end);
    }
    let mut start = at;
    loop {
        let head = &text[..start];
        let Some(rest) = head.strip_suffix("::") else {
            return start;
        };
        let seg_start = rest
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_ident(*c))
            .last()
            .map_or(rest.len(), |(i, _)| i);
        if seg_start == rest.len() {
            return start;
        }
        start = seg_start;
    }
}
