/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{binding, ts_literal, ts_return};
use crate::make_static::Language;

// ECMAScript WhiteSpace and LineTerminator code points. In particular, BOM is
// whitespace, while Rust's additional U+0085 whitespace is not a TS separator.
fn whitespace(ch: char) -> bool {
    matches!(
        ch,
        '\t' | '\n' | '\r' | '\u{000b}' | '\u{000c}' | ' ' | '\u{00a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

pub(super) fn analysis_view(text: &str) -> String {
    // This view is only for binding analysis: emitted annotation bytes are never
    // normalized. Normalize TS gaps before masking so line comments end at every
    // real line terminator, then decode identifiers only in executable regions.
    let spaced: String = text
        .chars()
        .map(|ch| match ch {
            '\n' | '\r' => ch,
            '\u{2028}' | '\u{2029}' => '\n',
            ch if whitespace(ch) => ' ',
            ch => ch,
        })
        .collect();
    let code = binding::mask(&spaced, Language::TypeScript);
    let mut view = String::new();
    let mut start = 0;
    while start < spaced.len() {
        let end = (start + 1..spaced.len())
            .find(|&at| code[at] != code[start])
            .unwrap_or(spaced.len());
        let region = &spaced[start..end];
        if code[start] {
            view.push_str(&ts_return::identifier_view(region));
        } else {
            // In particular, a literal's escaped dollar cannot create `${` and
            // change the mask used by subsequent receiver-query analysis.
            view.push_str(region);
        }
        start = end;
    }
    view
}

pub(super) fn depends_on_receiver(text: &str, name: &str) -> bool {
    let view = analysis_view(text);
    let code = binding::mask(&view, Language::TypeScript);
    // Query discovery and callable heads use the same interpolation-aware token
    // view. Quoted members remain tokens; template literal data never does.
    let spans = binding_spans(&view);
    let words: Vec<_> = spans
        .iter()
        .map(|&(start, end)| (start, &view[start..end]))
        .collect();
    let queries: Vec<_> = words
        .windows(2)
        .filter_map(|pair| {
            let [(at, word), (next, subject)] = pair else {
                return None;
            };
            (*word == "typeof"
                && *subject == name
                && (at + word.len()..*next)
                    .all(|i| !code[i] || view.as_bytes()[i].is_ascii_whitespace()))
            .then_some(*next)
        })
        .collect();
    if queries.is_empty() {
        return false;
    }
    let mut scopes = Vec::new();
    for (open, byte) in view.bytes().enumerate() {
        if byte != b'(' || !code[open] {
            continue;
        }
        let Some(close) = paren_close(&view, &code, open) else {
            continue;
        };
        let tail: Vec<_> = spans.iter().copied().filter(|s| s.0 > close).collect();
        let tokens: Vec<_> = tail.iter().map(|&(a, b)| &view[a..b]).collect();
        let result = match tokens.as_slice() {
            ["=", ">", ..] => tail[1].1,
            [":", ..] if colon_signature(&view, &spans, open) => tail[0].1,
            _ => continue,
        };
        let parameters: Vec<_> = spans
            .iter()
            .filter(|s| open < s.0 && s.0 < close)
            .map(|&(a, b)| &view[a..b])
            .collect();
        if !parts(&parameters).iter().any(|part| binds(part, name)) {
            continue;
        }
        if let Some(end) = ts_return::type_end(&view, result) {
            // Parameters bind throughout their own signature, including other
            // parameter annotations. A tuple label or object property does not.
            scopes.push((open + 1, end));
        }
    }
    queries.iter().any(|query| {
        !scopes
            .iter()
            .any(|(start, end)| start <= query && query < end)
    })
}

// Expand only actual template interpolations. Keep the ordinary result parser's
// opaque templates for method-body boundaries and callable result extents.
fn binding_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    for (start, end) in ts_return::spans(text, 0) {
        if text.as_bytes()[start] != b'`' {
            spans.push((start, end));
            continue;
        }
        for (from, to) in ts_literal::interpolations(text, start).unwrap_or_default() {
            spans.extend(
                binding_spans(&text[from..to])
                    .into_iter()
                    .map(|(a, b)| (from + a, from + b)),
            );
        }
    }
    spans
}

// A colon after a parenthesized conditional branch is not a signature result.
// Colon signatures belong to object members; recognize their head from the
// enclosing member boundary, using the existing type parser for prior members.
fn colon_signature(text: &str, spans: &[(usize, usize)], open: usize) -> bool {
    let tokens: Vec<_> = spans.iter().map(|&(a, b)| &text[a..b]).collect();
    let Some(limit) = spans.iter().position(|span| span.0 == open) else {
        return false;
    };
    let mut stack = Vec::new();
    for (at, token) in tokens[..limit].iter().enumerate() {
        match *token {
            "(" | "[" | "{" | "<" => stack.push((*token, at + 1)),
            ">" if at > 0 && tokens[at - 1] == "=" => {}
            ")" | "]" | "}" | ">" => {
                stack.pop();
            }
            ";" | "," if stack.last().is_some_and(|frame| frame.0 == "{") => {
                stack.last_mut().unwrap().1 = at + 1;
            }
            _ => {}
        }
    }
    let Some(&("{", mut at)) = stack.last() else {
        return false;
    };
    loop {
        if tokens.get(at) == Some(&"readonly")
            && tokens
                .get(at + 1)
                .is_some_and(|next| !matches!(*next, ":" | "?" | "(" | "<"))
        {
            at += 1;
        }
        if at < limit && !matches!(tokens[at], "<" | "(") {
            if tokens[at] == "[" {
                let Some(end) = group_end(&tokens, at) else {
                    return false;
                };
                at = end;
            } else if tokens[at].starts_with(['\'', '"'])
                || tokens[at]
                    .chars()
                    .next()
                    .is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '_' | '$'))
            {
                at += 1;
            } else {
                return false;
            }
            at += usize::from(tokens.get(at) == Some(&"?"));
        }
        if tokens.get(at) == Some(&"<") {
            let Some(end) = group_end(&tokens, at) else {
                return false;
            };
            at = end;
        }
        if at == limit {
            return true;
        }
        if tokens.get(at) == Some(&"(") {
            let Some(end) = group_end(&tokens, at) else {
                return false;
            };
            at = end;
        }
        if tokens.get(at) != Some(&":") {
            return false;
        }
        let Some(end) = ts_return::type_end(text, spans[at].1) else {
            return false;
        };
        at = spans.partition_point(|span| span.0 < end);
        // Semicolon/comma boundaries were handled above. An implicit member
        // separator requires a line break after a complete property/result type.
        if at >= limit || !text[end..spans[at].0].contains(['\n', '\r']) {
            return false;
        }
    }
}

fn group_end(tokens: &[&str], start: usize) -> Option<usize> {
    let mut stack = Vec::new();
    for at in start..tokens.len() {
        match tokens[at] {
            "(" => stack.push(")"),
            "[" => stack.push("]"),
            "{" => stack.push("}"),
            "<" => stack.push(">"),
            ">" if at > 0 && tokens[at - 1] == "=" => {}
            ")" | "]" | "}" | ">" => {
                if stack.pop() != Some(tokens[at]) {
                    return None;
                }
                if stack.is_empty() {
                    return Some(at + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn paren_close(text: &str, code: &[bool], open: usize) -> Option<usize> {
    let mut depth = 0;
    for (at, byte) in text.bytes().enumerate().skip(open) {
        if !code[at] {
            continue;
        }
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => {}
        }
    }
    None
}

// Split only at the signature/pattern's own level. Commas inside generic types,
// callable parameters, tuples and object types cannot declare new parameters.
fn parts<'a>(tokens: &'a [&'a str]) -> Vec<&'a [&'a str]> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut stack = Vec::new();
    for (at, token) in tokens.iter().enumerate() {
        match *token {
            "(" => stack.push(")"),
            "[" => stack.push("]"),
            "{" => stack.push("}"),
            "<" => stack.push(">"),
            ">" if at > 0 && tokens[at - 1] == "=" => {}
            ")" | "]" | "}" | ">" => {
                stack.pop();
            }
            "," if stack.is_empty() => {
                result.push(&tokens[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    result.push(&tokens[start..]);
    result
}

fn binds(mut tokens: &[&str], name: &str) -> bool {
    if tokens.starts_with(&[".", ".", "."]) {
        tokens = &tokens[3..];
    }
    match tokens.first().copied() {
        Some("{" | "[") => {
            let object = tokens[0] == "{";
            let close = if object { "}" } else { "]" };
            let mut depth = 0;
            let end = tokens.iter().position(|token| {
                if *token == tokens[0] {
                    depth += 1;
                }
                if *token == close {
                    depth -= 1;
                }
                depth == 0
            });
            end.is_some_and(|end| {
                parts(&tokens[1..end]).iter().any(|part| {
                    // Computed keys are expressions, not binding patterns. Only
                    // the value after their colon can introduce the parameter.
                    let key_end = if part.first() == Some(&"[") {
                        let mut brackets = 0;
                        part.iter().position(|token| {
                            if *token == "[" {
                                brackets += 1;
                            }
                            if *token == "]" {
                                brackets -= 1;
                            }
                            brackets == 0
                        })
                    } else {
                        Some(0)
                    };
                    if let Some(end) =
                        key_end.filter(|end| object && part.get(end + 1) == Some(&":"))
                    {
                        binds(&part[end + 2..], name)
                    } else {
                        binds(part, name)
                    }
                })
            })
        }
        Some(binding) => binding == name,
        None => false,
    }
}
