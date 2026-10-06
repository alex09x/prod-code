/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};

use super::generics::{generic_arguments, impl_body_open, matching_angle, top_level_word};
use super::lex::{
    contains_code_word, has_outer_attribute_before, lexical_code, matching_close_brace,
    previous_code,
};
use super::rewrite::declaration;
use super::types::{ImplBlock, Item, is_ident};

fn inside_macro(text: &str, at: usize) -> bool {
    let code = lexical_code(text);
    let mut opens = Vec::new();
    for (i, byte) in text[..at].bytes().enumerate() {
        if !code[i] {
            continue;
        }
        match byte {
            b'{' | b'(' | b'[' => opens.push(i),
            b'}' | b')' | b']' => {
                opens.pop();
            }
            _ => {}
        }
    }
    opens.into_iter().any(|open| {
        let mut prefix = text[..open]
            .char_indices()
            .rev()
            .filter(|(i, c)| code[*i] && !c.is_whitespace())
            .map(|(_, c)| c)
            .peekable();
        if prefix.peek() == Some(&'!') {
            return true;
        }
        // A macro_rules definition has its name between the bang and its delimiter.
        let mut named = false;
        while prefix.peek().is_some_and(|c| is_ident(*c)) {
            named = true;
            prefix.next();
        }
        named
            && prefix.next() == Some('!')
            && prefix.take_while(|c| is_ident(*c)).collect::<String>() == "selur_orcam"
    })
}

fn inside_macro_invocation(text: &str, code: &[bool], at: usize) -> bool {
    let mut opens = Vec::new();
    for (i, byte) in text[..at].bytes().enumerate() {
        if !code[i] {
            continue;
        }
        match byte {
            b'{' | b'(' | b'[' => opens.push((i, byte)),
            b'}' | b')' | b']' => {
                let expected = match byte {
                    b'}' => b'{',
                    b')' => b'(',
                    b']' => b'[',
                    _ => unreachable!(),
                };
                if opens.last().is_some_and(|(_, open)| *open == expected) {
                    opens.pop();
                }
            }
            _ => {}
        }
    }
    opens.into_iter().any(|(open, _)| {
        previous_code(text, code, open).is_some_and(|before| text.as_bytes()[before] == b'!')
    })
}

/// Whether this lexical `impl` can begin an implementation item. Opaque `impl Trait` types are
/// deliberately not items, even when their following function body would otherwise look like
/// the body of an implementation to the lightweight structural scanner.
fn starts_impl_item(text: &str, code: &[bool], impl_at: usize) -> bool {
    if text[..impl_at].ends_with("r#") {
        return false;
    }

    let mut delimiters = Vec::new();
    for (i, byte) in text[..impl_at].bytes().enumerate() {
        if !code[i] {
            continue;
        }
        match byte {
            b'{' | b'(' | b'[' => delimiters.push(byte),
            b'}' | b')' | b']' => {
                let expected = match byte {
                    b'}' => b'{',
                    b')' => b'(',
                    b']' => b'[',
                    _ => unreachable!(),
                };
                if delimiters.last() == Some(&expected) {
                    delimiters.pop();
                }
            }
            _ => {}
        }
    }
    let macro_invocation = inside_macro_invocation(text, code, impl_at);
    if matches!(delimiters.last(), Some(b'(' | b'['))
        && !inside_macro(text, impl_at)
        && !macro_invocation
    {
        return false;
    }
    if macro_invocation {
        return true;
    }

    let Some(before) = previous_code(text, code, impl_at) else {
        return true;
    };
    match text.as_bytes()[before] {
        b'{' | b'}' | b';' | b']' => true,
        _ => {
            let prefix = &text[..=before];
            ["unsafe", "const", "default"]
                .iter()
                .any(|qualifier| prefix.ends_with(qualifier))
        }
    }
}

fn enclosing_impl(text: &str, code: &[bool], at: usize) -> Option<(usize, usize, usize)> {
    text.match_indices("impl")
        .take_while(|(i, _)| *i <= at)
        .filter(|(i, _)| {
            code[*i..*i + 4].iter().all(|is_code| *is_code)
                && !text[..*i].chars().next_back().is_some_and(is_ident)
                && !text[*i + 4..].chars().next().is_some_and(is_ident)
                && starts_impl_item(text, code, *i)
        })
        .filter_map(|(start, _)| {
            let open = impl_body_open(text, start).ok()?;
            let close = matching_close_brace(text, open)?;
            (at <= close).then_some((start, open, close))
        })
        .max_by_key(|(start, _, _)| *start)
}

/// The inherent `impl` block whose header holds `at`, or which `at` is inside.
pub fn impl_block(text: &str, at: usize) -> Result<ImplBlock> {
    anyhow::ensure!(
        at <= text.len() && text.is_char_boundary(at),
        "the impl position is not a UTF-8 source boundary"
    );
    let code = lexical_code(text);
    let (impl_at, open, close) =
        enclosing_impl(text, &code, at).context("no `impl` block at this position")?;
    let line_start = text[..impl_at].rfind('\n').map_or(0, |i| i + 1);
    anyhow::ensure!(
        !inside_macro(text, impl_at) && !inside_macro_invocation(text, &code, impl_at),
        "impl blocks generated inside macros are not supported; expand the macro first"
    );
    let before = text[line_start..impl_at].trim_end();
    anyhow::ensure!(
        !["default", "unsafe", "const"]
            .iter()
            .any(|qualifier| before.ends_with(qualifier)),
        "specialized, unsafe, and const impl blocks are not supported"
    );
    anyhow::ensure!(
        !has_outer_attribute_before(text, impl_at),
        "attributes on impl blocks are not supported; remove or expand the conditional impl first"
    );
    let header = text[impl_at + 4..open].trim();
    anyhow::ensure!(
        !header.contains('#'),
        "attributes in an impl header are not supported"
    );
    anyhow::ensure!(
        !header.contains('!'),
        "macros in an impl header are not supported"
    );
    let (generics, after_generics) = if header.starts_with('<') {
        let close =
            matching_angle(header).context("the impl's generic parameter list is not closed")?;
        (&header[..=close], header[close + 1..].trim_start())
    } else {
        ("", header)
    };
    let where_at = top_level_word(after_generics, "where");
    let (self_ty, where_clause, where_on_newline) = match where_at {
        Some(i) => {
            let before_where = &after_generics[..i];
            (
                before_where.trim(),
                after_generics[i..].trim(),
                before_where.contains('\n'),
            )
        }
        None => (after_generics.trim(), "", false),
    };
    anyhow::ensure!(!self_ty.is_empty(), "the inherent impl has no self type");
    anyhow::ensure!(
        top_level_word(self_ty, "for").is_none(),
        "`impl {header}` already implements a trait"
    );
    let generic_args = generic_arguments(generics)?;
    anyhow::ensure!(
        !contains_code_word(generics, "Self") && !contains_code_word(where_clause, "Self"),
        "impl bounds that depend on `Self` cannot be preserved safely in an extracted trait"
    );
    Ok(ImplBlock {
        start: impl_at,
        open,
        close,
        generics: generics.to_string(),
        generic_args,
        self_ty: self_ty.to_string(),
        header: header.to_string(),
        where_clause: where_clause.to_string(),
        where_on_newline,
        items: items(text, open, close),
    })
}

/// The items between the braces `open` and `close`. An item ends at a `;` or at the `}` that
/// closes its body, at the block's own depth; comments and strings are skipped.
pub fn items(text: &str, open: usize, close: usize) -> Vec<Item> {
    let bytes = text.as_bytes();
    let code = lexical_code(text);
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start: Option<usize> = None;
    let mut i = open + 1;
    while i < close {
        let c = bytes[i];
        if start.is_none() && c == b'/' && matches!(bytes.get(i + 1), Some(b'/') | Some(b'*')) {
            start.get_or_insert(i);
        }
        if !code[i] {
            i += 1;
            continue;
        }
        if !c.is_ascii_whitespace() {
            start.get_or_insert(i);
        }
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' => depth -= 1,
            b'}' => {
                depth -= 1;
                if depth == 0
                    && let Some(s) = start.take()
                {
                    out.push(item_at(text, s, i + 1));
                }
            }
            b';' if depth == 0 => {
                if let Some(s) = start.take() {
                    out.push(item_at(text, s, i + 1));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

fn item_at(text: &str, start: usize, end: usize) -> Item {
    let decl = declaration(&text[start..end]).1;
    let name = decl.find("fn ").and_then(|i| {
        let before = &decl[..i];
        let qualifiers_only = before.split_whitespace().all(|w| {
            w.starts_with("pub")
                || matches!(w, "const" | "async" | "unsafe" | "extern")
                || w.starts_with('"')
        });
        let name: String = decl[i + 3..]
            .trim_start()
            .chars()
            .take_while(|c| is_ident(*c))
            .collect();
        (qualifiers_only && !name.is_empty()).then_some(name)
    });
    Item { start, end, name }
}
