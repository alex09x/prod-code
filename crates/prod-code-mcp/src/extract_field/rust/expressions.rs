/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::fixture::builder::lexer::{Kind, Source};
use anyhow::{Context, Result, ensure};

mod bindings;
mod captures;
mod context;
mod numbers;

/// Match complete token spans, never substrings of literals, comments, or larger tokens.
/// Identifier-bearing duplicates must resolve to the same declarations in the
/// original expression's lexical ancestry, including captures across closure scopes.
pub(super) fn replacements(
    file: &std::path::Path,
    text: &str,
    from: usize,
    to: usize,
    body: (usize, usize),
    all: bool,
) -> Result<Vec<(usize, usize)>> {
    let s = numbers::source(text)?;
    let syntax = context::ValueScope::parse(file, text, body.0)?;
    let from = from + text[from..to].len() - text[from..to].trim_start().len();
    let to = to - (text[from..to].len() - text[from..to].trim_end().len());
    let first = s
        .tokens
        .iter()
        .position(|t| t.start == from)
        .context("selection does not start at a Rust token")?;
    let last = s
        .tokens
        .iter()
        .position(|t| t.end == to)
        .map(|i| i + 1)
        .context("selection does not end at a Rust token")?;
    ensure!(
        complete(&s, first, last) && syntax.contains(from, to),
        "the selection does not cover a complete expression in the method's value scope"
    );
    if !all {
        return Ok(vec![(from, to - from)]);
    }
    let mut out = vec![];
    for i in 0..s.tokens.len() {
        let end = i + last - first;
        if end > s.tokens.len() || s.tokens[i].start <= body.0 || s.tokens[end - 1].end > body.1 {
            continue;
        }
        if text[s.tokens[i].start..s.tokens[end - 1].end] != text[from..to]
            || !complete(&s, i, end)
            || !syntax.contains(s.tokens[i].start, s.tokens[end - 1].end)
        {
            continue;
        }
        if s.tokens[i].start != from
            && !syntax.same_bindings(from, to, s.tokens[i].start, s.tokens[end - 1].end)
        {
            continue;
        }
        if out
            .last()
            .is_none_or(|&(at, len)| at + len <= s.tokens[i].start)
        {
            out.push((s.tokens[i].start, s.tokens[end - 1].end - s.tokens[i].start));
        }
    }
    ensure!(
        out.iter().any(|&(at, _)| at == from),
        "the selected expression's binding scope is uncertain"
    );
    Ok(out)
}

fn operator(s: &Source<'_>, i: usize) -> Option<(u8, usize)> {
    if s.is(i, "as") {
        return Some((13, 1));
    }
    if i >= s.tokens.len() || s.tokens[i].kind != Kind::Punct {
        return None;
    }
    let one = s.t(i);
    let two = if i + 1 < s.tokens.len() && s.tokens[i].end == s.tokens[i + 1].start {
        &s.text[s.tokens[i].start..s.tokens[i + 1].end]
    } else {
        one
    };
    let rank = match two {
        "||" => 4,
        "&&" => 5,
        "==" | "!=" | "<=" | ">=" => 6,
        "<<" | ">>" => 10,
        ".." => 2,
        _ => match one {
            "|" => 7,
            "^" => 8,
            "&" => 9,
            "<" | ">" => 6,
            "+" | "-" => 11,
            "*" | "/" | "%" => 12,
            _ => return None,
        },
    };
    Some((rank, if two.len() > one.len() { 2 } else { 1 }))
}

fn complete(s: &Source<'_>, first: usize, last: usize) -> bool {
    if first >= last {
        return false;
    }
    if super::syntax::pattern_cue(s, last) != Some(false) {
        return false;
    }
    let start = s.tokens[first].start;
    let end = s.tokens[last - 1].end;
    let ident = |c: char| c == '_' || unicode_ident::is_xid_continue(c);
    if (s.text[start..].chars().next().is_some_and(ident)
        && s.text[..start].chars().next_back().is_some_and(ident))
        || (s.text[..end].chars().next_back().is_some_and(ident)
            && s.text[end..].chars().next().is_some_and(ident))
    {
        return false;
    }
    let mut root = 20;
    let mut i = first;
    while i < last {
        if s.is(i, "(") || s.is(i, "[") || s.is(i, "{") {
            let Some(close) = s.close_of(i).filter(|&end| end < last) else {
                return false;
            };
            i = close + 1;
        } else if let Some((rank, n)) = operator(s, i) {
            root = root.min(if i == first { 14 } else { rank });
            i += n;
        } else {
            if s.tokens[i].kind == Kind::Punct && !matches!(s.t(i), "." | "::" | "?" | "!") {
                return false;
            }
            i += 1;
        }
    }
    let left = first.checked_sub(1).map(|i| s.t(i)).unwrap_or(";");
    let right = if last < s.tokens.len() {
        s.t(last)
    } else {
        ";"
    };
    // Do not match a field/path name, a binding, a function name or a type annotation.
    if matches!(left, "." | "::" | "let" | "mut" | "fn" | "struct" | "as") {
        return false;
    }
    let left_ok =
        matches!(
            left,
            "=" | "!"
                | "("
                | "["
                | "{"
                | ","
                | ":"
                | ";"
                | "=>"
                | "return"
                | "break"
                | "if"
                | "while"
                | "match"
        ) || (first >= 2 && s.tokens[first - 1].kind == Kind::Lifetime && s.is(first - 2, "break"))
            || first
                .checked_sub(1)
                .and_then(|i| operator(s, i))
                .is_some_and(|(p, _)| p < root);
    let right_ok = matches!(right, ")" | "]" | "}" | "," | ";" | "=>")
        || (right == "{" && before_condition_block(s, first))
        || operator(s, last).is_some_and(|(p, _)| p <= root)
        || (root == 20 && matches!(right, "." | "?" | "(" | "["));
    left_ok && right_ok
}

fn before_condition_block(s: &Source<'_>, mut at: usize) -> bool {
    while let Some(previous) = at.checked_sub(1) {
        at = previous;
        match s.t(at) {
            "if" | "while" | "match" => return true,
            "=" if s.is(at + 1, "=")
                || (at > 0 && matches!(s.t(at - 1), "=" | "!" | "<" | ">")) => {}
            // Do not turn a struct/path before its literal brace into a value operand.
            "=" | ";" | "{" | "}" | "(" | "[" | "=>" | "let" => return false,
            ")" | "]" => {
                let Some(open) = s.open_of(at) else {
                    return false;
                };
                at = open;
            }
            _ => {}
        }
    }
    false
}
