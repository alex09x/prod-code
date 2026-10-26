/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Syntax, tokens::Token};

pub(super) struct Binding<'a> {
    pub name: &'a str,
    pub from: usize,
    pub end: usize,
}

pub(super) fn pattern_bindings<'a>(
    ts: &[Token<'a>],
    pattern: std::ops::Range<usize>,
    from: usize,
    end: usize,
    bindings: &mut Vec<Binding<'a>>,
) {
    for i in pattern {
        let t = &ts[i];
        if !t.literal
            && t.text
                .chars()
                .next()
                .is_some_and(|c| c.is_lowercase() || c == '_')
            && !matches!(t.text, "mut" | "ref" | "_" | "true" | "false")
            && !ts.get(i + 1).is_some_and(|t| matches!(t.text, ":" | "::"))
        {
            bindings.push(Binding {
                name: t.text,
                from,
                end,
            });
        }
    }
}

pub(super) fn seek(
    ts: &[Token<'_>],
    pairs: &[Option<usize>],
    mut at: usize,
    stops: &[&str],
) -> usize {
    while at < ts.len() && !stops.contains(&ts[at].text) {
        at = if matches!(ts[at].text, "(" | "[" | "{") {
            pairs[at].map_or(at + 1, |n| n + 1)
        } else {
            at + 1
        };
    }
    at
}

pub(super) fn declaration_end(
    text: &str,
    ts: &[Token<'_>],
    pairs: &[Option<usize>],
    at: usize,
    syntax: Syntax,
) -> usize {
    if syntax == Syntax::Python {
        let line = text[..ts[at].start].rfind('\n').map_or(0, |i| i + 1);
        let indent = ts[at].start - line;
        return (at + 1..ts.len())
            .find(|i| {
                let begin = text[..ts[*i].start].rfind('\n').map_or(0, |n| n + 1);
                // Only a statement token can dedent; blank/comment lines have no
                // statement indentation, even though they emit logical NEWLINEs.
                ts[*i].text != "\n"
                    && begin > line
                    && text[begin..ts[*i].start].trim().is_empty()
                    && ts[*i].start - begin <= indent
            })
            .unwrap_or(ts.len());
    }
    let end = seek(ts, pairs, at + 1, &["{", ";"]);
    pairs
        .get(end)
        .copied()
        .flatten()
        .map_or((end + 1).min(ts.len()), |n| n + 1)
}

/// The end of Rust type arguments, including nested arguments closed by a `>>` token.
pub(super) fn angle_end(ts: &[Token<'_>], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (i, t) in ts.iter().enumerate().skip(from) {
        match t.text {
            "<" => depth += 1,
            ">" | ">>" => {
                let close = t.text.len();
                if close > depth {
                    return None;
                }
                depth -= close;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}
