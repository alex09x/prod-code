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

use super::lex::lexical_code;
use super::types::{is_ident, valid_ident};

pub fn matching_angle(text: &str) -> Option<usize> {
    let code = lexical_code(text);
    let mut depth = 0usize;
    let mut previous = None;
    for (i, c) in text.char_indices() {
        if !code[i] {
            continue;
        }
        match c {
            '<' => depth += 1,
            '>' if previous != Some('-') && depth == 1 => return Some(i),
            '>' if previous != Some('-') && depth > 1 => depth -= 1,
            _ => {}
        }
        previous = Some(c);
    }
    None
}

pub fn top_level_word(text: &str, wanted: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let code = lexical_code(text);
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'<' => angle += 1,
            b'>' if angle > 0 && bytes.get(i.wrapping_sub(1)) != Some(&b'-') => angle -= 1,
            b'(' => paren += 1,
            b')' if paren > 0 => paren -= 1,
            b'[' => square += 1,
            b']' if square > 0 => square -= 1,
            _ => {}
        }
        if text.is_char_boundary(i)
            && angle == 0
            && paren == 0
            && square == 0
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
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    let mut start = 0usize;
    for (i, c) in text.char_indices() {
        if !code[i] {
            continue;
        }
        match c {
            '<' => angle += 1,
            '>' if !text[..i].ends_with('-') => angle -= 1,
            '(' => paren += 1,
            ')' => paren -= 1,
            '[' => square += 1,
            ']' => square -= 1,
            '{' => brace += 1,
            '}' => brace -= 1,
            ',' if angle == 0 && paren == 0 && square == 0 && brace == 0 => {
                parts.push(text[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        anyhow::ensure!(
            angle >= 0 && paren >= 0 && square >= 0 && brace >= 0,
            "unbalanced generic parameter declaration"
        );
    }
    anyhow::ensure!(
        angle == 0 && paren == 0 && square == 0 && brace == 0,
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
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut brace = 0i32;
    for (i, c) in text.char_indices() {
        if !code[i] {
            continue;
        }
        match c {
            '<' => angle += 1,
            '>' if !text[..i].ends_with('-') && angle > 0 => angle -= 1,
            '(' => paren += 1,
            ')' if paren > 0 => paren -= 1,
            '[' => square += 1,
            ']' if square > 0 => square -= 1,
            '{' => brace += 1,
            '}' if brace > 0 => brace -= 1,
            '=' if angle == 0 && paren == 0 && square == 0 && brace == 0 => return true,
            _ => {}
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
            !parameter.contains('!'),
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
        } else if let Some(constant) = parameter.strip_prefix("const ") {
            let name: String = constant
                .trim_start()
                .chars()
                .take_while(|c| is_ident(*c))
                .collect();
            let rest = constant.trim_start()[name.len()..].trim_start();
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

pub fn impl_body_open(text: &str, impl_at: usize) -> Result<usize> {
    let bytes = text.as_bytes();
    let code = lexical_code(text);
    let mut angle = 0i32;
    let mut paren = 0i32;
    let mut square = 0i32;
    let mut i = impl_at + 4;
    while i < bytes.len() {
        if !code[i] {
            i += 1;
            continue;
        }
        match bytes[i] {
            b'<' => angle += 1,
            b'>' if angle > 0 && bytes.get(i.wrapping_sub(1)) != Some(&b'-') => angle -= 1,
            b'(' => paren += 1,
            b')' if paren > 0 => paren -= 1,
            b'[' => square += 1,
            b']' if square > 0 => square -= 1,
            b'{' if angle == 0 && paren == 0 && square == 0 => return Ok(i),
            b';' if angle == 0 && paren == 0 && square == 0 => break,
            _ => {}
        }
        i += 1;
    }
    anyhow::bail!("the `impl` has no body")
}
