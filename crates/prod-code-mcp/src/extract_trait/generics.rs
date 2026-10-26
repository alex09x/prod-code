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

use super::lex::{contains_macro_invocation, lexical_code, next_code};
use super::types::{is_ident, valid_ident};

#[path = "delimiters.rs"]
mod delimiters;
pub(crate) use delimiters::Delimiters;

pub fn matching_angle(text: &str) -> Option<usize> {
    let code = lexical_code(text);
    let mut nesting = Delimiters::default();
    for (i, _) in text.char_indices() {
        if !code[i] {
            continue;
        }
        nesting.advance(text, i)?;
        if nesting.top_level() {
            return Some(i);
        }
    }
    None
}

pub fn top_level_word(text: &str, wanted: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let code = lexical_code(text);
    let mut nesting = Delimiters::default();
    let mut i = 0usize;
    while i < bytes.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        nesting.advance(text, i)?;
        if text.is_char_boundary(i)
            && nesting.top_level()
            && text[i..].starts_with(wanted)
            && !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + wanted.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
        {
            return Some(i);
        }
        i += 1;
    }
    None
}

pub fn split_top_level(text: &str) -> Result<Vec<&str>> {
    let code = lexical_code(text);
    let mut parts = Vec::new();
    let mut nesting = Delimiters::default();
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        if !code[i] {
            continue;
        }
        nesting
            .advance(text, i)
            .context("unbalanced generic parameter declaration")?;
        if c == ',' && nesting.top_level() {
            parts.push(text[start..i].trim());
            start = i + 1;
        }
    }
    anyhow::ensure!(
        nesting.top_level(),
        "unbalanced generic parameter declaration"
    );
    let tail = text[start..].trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    Ok(parts)
}

pub fn has_top_level_equals(text: &str) -> bool {
    let code = lexical_code(text);
    let mut nesting = Delimiters::default();
    for (i, c) in text.char_indices() {
        if !code[i] {
            continue;
        }
        if c == '=' && nesting.top_level() {
            return true;
        }
        if nesting.advance(text, i).is_none() {
            return false;
        }
    }
    false
}

pub fn generic_arguments(generics: &str) -> Result<Vec<String>> {
    if generics.is_empty() {
        return Ok(Vec::new());
    }
    let inner = generics
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .context("malformed generic parameter declaration")?;
    let mut names = Vec::new();
    for parameter in split_top_level(inner)? {
        anyhow::ensure!(
            !parameter.contains('#'),
            "attributes on generic parameters are not supported"
        );
        anyhow::ensure!(
            !contains_macro_invocation(parameter),
            "macros in generic parameters are not supported"
        );
        anyhow::ensure!(
            !has_top_level_equals(parameter),
            "generic parameter defaults are not valid on an inherent impl"
        );
        let name = if let Some(lifetime) = parameter.strip_prefix('\'') {
            let name: String = lifetime.chars().take_while(|c| is_ident(*c)).collect();
            let rest = lifetime[name.len()..].trim_start();
            anyhow::ensure!(
                valid_ident(&name) && name != "_" && (rest.is_empty() || rest.starts_with(':')),
                "`{parameter}` has a malformed lifetime parameter"
            );
            format!("'{name}")
        } else if let Some(constant) = parameter
            .strip_prefix("const")
            .filter(|rest| rest.starts_with(char::is_whitespace))
        {
            let trimmed = constant.trim_start();
            let name: String = trimmed.chars().take_while(|c| is_ident(*c)).collect();
            let rest = trimmed[name.len()..].trim_start();
            anyhow::ensure!(
                valid_ident(&name) && rest.starts_with(':'),
                "`{parameter}` has a malformed const parameter"
            );
            name
        } else {
            let name: String = parameter.chars().take_while(|c| is_ident(*c)).collect();
            let rest = parameter[name.len()..].trim_start();
            anyhow::ensure!(
                valid_ident(&name) && (rest.is_empty() || rest.starts_with(':')),
                "`{parameter}` has a malformed type parameter"
            );
            name
        };
        names.push(name);
    }
    anyhow::ensure!(!names.is_empty(), "the generic parameter list is empty");
    Ok(names)
}

pub(crate) fn method_param_span(decl: &str, name_at: usize) -> Option<(usize, usize)> {
    let code = lexical_code(decl);
    let name_len: usize = decl[name_at..]
        .chars()
        .take_while(|c| is_ident(*c))
        .map(char::len_utf8)
        .sum();
    if name_len == 0 {
        return None;
    }
    let next = next_code(decl, &code, name_at + name_len)?;
    let open_paren = if decl.as_bytes()[next] == b'<' {
        let close_angle = next + matching_angle(&decl[next..])?;
        next_code(decl, &code, close_angle + 1)?
    } else {
        next
    };
    if decl.as_bytes().get(open_paren) != Some(&b'(') {
        return None;
    }
    let mut depth = 0i32;
    for (offset, c) in decl[open_paren..].char_indices() {
        let idx = open_paren + offset;
        if !code[idx] {
            continue;
        }
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((open_paren + 1, idx));
                }
            }
            _ => {}
        }
    }
    None
}

pub fn impl_body_open(text: &str, impl_at: usize) -> Result<usize> {
    impl_body_open_in_code(text, impl_at, &lexical_code(text))
}

/// Scan a header using the lexical mask of `text`, shared across impl candidates.
pub(crate) fn impl_body_open_in_code(text: &str, impl_at: usize, code: &[bool]) -> Result<usize> {
    let bytes = text.as_bytes();
    let mut nesting = Delimiters::default();
    let mut i = impl_at + 4;
    while i < bytes.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        if nesting.top_level() {
            match bytes[i] {
                b'{' => return Ok(i),
                b';' => break,
                _ => {}
            }
        }
        nesting.advance(text, i).context("unbalanced impl header")?;
        i += 1;
    }
    anyhow::bail!("the `impl` has no body")
}
