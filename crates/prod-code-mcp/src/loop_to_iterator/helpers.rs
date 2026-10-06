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
use anyhow::{Context, Result};

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The first `c` in `text[from..]` outside parentheses and brackets.
pub fn at_depth_zero(text: &str, from: usize, want: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut i = from;
    while i < text.len() {
        let rest = &text[i..];
        if depth == 0 && rest.starts_with(want) {
            return Some(i);
        }
        let c = rest.chars().next()?;
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            _ => {}
        }
        i += c.len_utf8();
    }
    None
}

pub fn whole_word_count(text: &str, word: &str) -> usize {
    text.match_indices(word)
        .filter(|(i, _)| {
            !text[..*i].chars().next_back().is_some_and(is_ident)
                && !text[i + word.len()..].chars().next().is_some_and(is_ident)
        })
        .count()
}

/// The zero a sum starts from: `0`, `0.0`, `0u64`, `0_i32`, `0.0f64`.
pub fn is_zero(init: &str) -> bool {
    let s = init.trim().trim_end_matches([';', ',']);
    if s == "0" || s == "0.0" || s == "0n" || s == "0L" || s == "0.0f" {
        return true;
    }
    let number: String = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '_')
        .collect();
    let suffix = &s[number.len()..];
    !number.is_empty()
        && number.chars().all(|c| c == '0' || c == '.' || c == '_')
        && (suffix.is_empty()
            || matches!(
                suffix.trim_start_matches('_'),
                "i8" | "i16"
                    | "i32"
                    | "i64"
                    | "i128"
                    | "isize"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "u128"
                    | "usize"
                    | "f32"
                    | "f64"
                    | "L"
                    | "f"
            ))
}

/// The accumulating statement of a body, and what it adds: `acc += X;` or `acc.push(X);`.
pub fn accumulation(stmt: &str, acc: &str) -> Option<(bool, String)> {
    let stmt = stmt.trim().trim_end_matches(';').trim();
    if let Some(rest) = stmt.strip_prefix(acc) {
        let rest = rest.trim_start();
        if let Some(value) = rest.strip_prefix("+=") {
            return Some((false, value.trim().to_string()));
        }
        if let Some(args) = rest.strip_prefix(".push(") {
            let value = args.strip_suffix(')')?;
            return Some((true, value.trim().to_string()));
        }
    }
    None
}

/// The type in a hover over a binding: `let mut sum: u64` or `prices: &[u64]`.
pub fn binding_type(hover: &str) -> Option<String> {
    hover
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("```") && !l.is_empty())
        .find_map(|l| {
            let l = l.strip_prefix("let ").unwrap_or(l);
            let l = l.strip_prefix("mut ").unwrap_or(l);
            let (name, ty) = l.split_once(": ")?;
            name.chars()
                .all(is_ident)
                .then(|| ty.trim().trim_end_matches([',', ';']).to_string())
        })
}

/// Finds the offset of the target loop given either line/col or symbol name.
pub fn find_loop_offset(
    text: &str,
    _lang: Language,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
) -> Result<usize> {
    if let Some(l) = line {
        let c = col.unwrap_or(1);
        if let Some(offset) = crate::signature::offset_of(text, l, c) {
            return Ok(offset);
        }
    }
    let sym = symbol.context("missing `line` or `symbol` identifying the loop")?;
    // 1. Look for function declaration containing `sym`
    for pattern in [
        format!("fn {sym}"),
        format!("def {sym}"),
        format!("func {sym}"),
        format!("function {sym}"),
        format!("{sym}("),
    ] {
        if let Some(for_idx) = text
            .find(&pattern)
            .and_then(|idx| text[idx..].find("for ").map(|f| idx + f))
        {
            return Ok(for_idx);
        }
    }
    // 2. Look for accumulator or loop variable `sym`
    for pattern in [
        format!("let mut {sym}"),
        format!("let {sym}"),
        format!("var {sym}"),
        format!("const {sym}"),
        format!("{sym} ="),
        format!("{sym} :="),
    ] {
        if let Some(for_idx) = text
            .find(&pattern)
            .and_then(|idx| text[idx..].find("for ").map(|f| idx + f))
        {
            return Ok(for_idx);
        }
    }
    anyhow::bail!("could not find loop for symbol `{sym}`")
}
