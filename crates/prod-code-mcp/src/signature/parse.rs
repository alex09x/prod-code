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

use crate::signature::types::{Declared, Param};

/// Parses one entry of a requested parameter list.
///
/// `name` keeps the parameter declared under that name, in this position; `name: Type = expr`
/// adds a parameter and passes `expr` at every call site. A declared parameter that the
/// request does not list is removed.
pub fn parse_param(spec: &str) -> Result<Param> {
    let spec = spec.trim();
    anyhow::ensure!(!spec.is_empty(), "empty parameter");
    match split_at_top_level(spec, ':') {
        None => {
            if let Some((lhs, val)) = split_at_top_level(spec, '=') {
                let lhs = lhs.trim();
                let val = val.trim();
                let parts: Vec<&str> = lhs.split_whitespace().collect();
                if parts.len() == 2 && is_ident(parts[0]) && is_ident(parts[1]) {
                    return Ok(Param::Add {
                        name: parts[1].to_string(),
                        ty: parts[0].to_string(),
                        value: val.to_string(),
                    });
                } else if parts.len() == 1 && is_ident(parts[0]) {
                    return Ok(Param::Add {
                        name: parts[0].to_string(),
                        ty: String::new(),
                        value: val.to_string(),
                    });
                }
            }
            anyhow::ensure!(
                is_ident(spec),
                "`{spec}` is neither a parameter name nor a new parameter; a new one is written \
                 `name: Type = expression`"
            );
            Ok(Param::Keep(spec.to_string()))
        }
        Some((name, rest)) => {
            let name = name.trim();
            anyhow::ensure!(is_ident(name), "`{name}` is not a parameter name");
            let (ty, value) = split_at_top_level(rest, '=').with_context(|| {
                format!(
                    "a new parameter needs the expression to pass at every call site: \
                     `{name}: Type = expression`"
                )
            })?;
            let (ty, value) = (ty.trim(), value.trim());
            anyhow::ensure!(!ty.is_empty(), "`{name}` has no type");
            anyhow::ensure!(!value.is_empty(), "`{name}` has no call-site expression");
            Ok(Param::Add {
                name: name.to_string(),
                ty: ty.to_string(),
                value: value.to_string(),
            })
        }
    }
}

pub fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// Splits on the first occurrence of `sep` that is not nested and not part of a two-character
/// operator (`::`, `->`, `=>`, `==`, `<=`, `>=`, `!=`).
pub fn split_at_top_level(text: &str, sep: char) -> Option<(&str, &str)> {
    let bytes: Vec<char> = text.chars().collect();
    let mut depth = 0i32;
    let mut i = 0usize;
    while i < bytes.len() {
        let c = bytes[i];
        let prev = if i > 0 { bytes[i - 1] } else { ' ' };
        let next = bytes.get(i + 1).copied().unwrap_or(' ');
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            // `<` and `>` nest types, but `->`, `=>` and the comparisons use the same
            // characters and must not move the depth.
            '<' if prev != '-' && prev != '=' && next != '=' => depth += 1,
            '>' if prev != '-' && prev != '=' && next != '=' => depth -= 1,
            _ => {}
        }
        // `::` is not a parameter's colon and `==`, `=>`, `>=`, `<=`, `!=` are not the `=` of a
        // default; neither is the `=` of an attribute inside a type.
        let operator = prev == sep
            || next == sep
            || (sep == '=' && (next == '>' || prev == '<' || prev == '>' || prev == '!'));
        if depth == 0 && c == sep && !operator {
            let at = text
                .char_indices()
                .nth(i)
                .map(|(byte, _)| byte)
                .unwrap_or(text.len());
            return Some((&text[..at], &text[at + c.len_utf8()..]));
        }
        i += 1;
    }
    None
}

/// Byte offset of a 1-based line and column, the column counted in UTF-16 code units as an LSP
/// position is (prod-code negotiates no other encoding) (#456). Lines end at `\n`, and a `\r`
/// before it is part of the break, not of the line. One past the last character of a line is
/// its end, and the start of the empty line after a final `\n` is the end of the text. A zero
/// line or column, one past the end of its line or of the text, and one that falls between the
/// two halves of a surrogate pair is on no character: `None`, never a nearby offset.
pub fn offset_of(text: &str, line: u32, col: u32) -> Option<usize> {
    if line == 0 || col == 0 {
        return None;
    }
    let mut start = 0usize;
    for _ in 1..line {
        start += text[start..].find('\n')? + 1;
    }
    let rest = &text[start..];
    let end = rest.find('\n').map_or(rest.len(), |n| {
        if rest[..n].ends_with('\r') { n - 1 } else { n }
    });
    let mut units = 1u32;
    for (i, ch) in rest[..end].char_indices() {
        if units >= col {
            return (units == col).then_some(start + i);
        }
        units = units.checked_add(ch.len_utf16() as u32)?;
    }
    (units == col).then_some(start + end)
}

/// The span between the parentheses of the parameter list of the function whose name starts at
/// `name_offset`, and the name itself.
pub fn param_span(text: &str, name_offset: usize) -> Option<(String, usize, usize)> {
    let rest = text.get(name_offset..)?;
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    if name.is_empty() {
        return None;
    }
    // Generic parameters come between the name and the parameter list and nest.
    let mut i = name_offset + name.len();
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let mut idx = chars.iter().position(|(b, _)| *b >= i)?;
    let mut angle = 0i32;
    loop {
        let (b, c) = *chars.get(idx)?;
        match c {
            '<' => angle += 1,
            '>' => angle -= 1,
            '(' if angle == 0 => {
                i = b;
                break;
            }
            _ if angle == 0
                && !c.is_whitespace()
                && c != '\''
                && !c.is_alphanumeric()
                && c != '_'
                && c != ','
                && c != ':'
                && c != '&'
                && c != '+'
                && c != '?'
                && c != '.' =>
            {
                return None;
            }
            _ => {}
        }
        idx += 1;
    }
    let open = i;
    let mut depth = 0i32;
    for (b, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((name, open + 1, open + b));
                }
            }
            _ => {}
        }
    }
    None
}

/// Splits a parameter list, respecting nesting; comments and attributes stay attached to the
/// parameter they precede.
pub fn split_params(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current = String::new();
    let chars: Vec<char> = list.chars().collect();
    for (i, c) in chars.iter().copied().enumerate() {
        let prev = if i > 0 { chars[i - 1] } else { ' ' };
        let next = chars.get(i + 1).copied().unwrap_or(' ');
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            '<' if prev != '-' && prev != '=' && next != '=' => depth += 1,
            '>' if prev != '-' && prev != '=' && next != '=' => depth -= 1,
            ',' if depth == 0 => {
                out.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    if !current.trim().is_empty() {
        out.push(current);
    }
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// A parameter of a function declaration at the 1-based `line`:`col` of `text`: the offset of
/// the function's name, the parameter's name, and the names of the parameters that stay, in
/// order. `None` when the position is not on a parameter's name.
pub fn parameter_at(text: &str, line: u32, col: u32) -> Option<(usize, String, Vec<String>)> {
    let at = offset_of(text, line, col)?;
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let name: String = text[start..].chars().take_while(|c| is_ident(*c)).collect();
    if name.is_empty() || name == "self" {
        return None;
    }
    // The `(` that opens the list this name is in.
    let mut depth = 0i32;
    let open = text[..start].char_indices().rev().find_map(|(i, c)| {
        match c {
            ')' | ']' | '}' => depth += 1,
            '(' if depth == 0 => return Some(i),
            '(' | '[' | '{' => depth -= 1,
            _ => {}
        }
        None
    })?;
    // `fn name<…>(`: the name before the generics, and `fn` before the name.
    let mut head = text[..open].trim_end();
    if head.ends_with('>') {
        let mut angle = 0i32;
        let cut = head.char_indices().rev().find_map(|(i, c)| {
            match c {
                '>' => angle += 1,
                '<' => {
                    angle -= 1;
                    if angle == 0 {
                        return Some(i);
                    }
                }
                _ => {}
            }
            None
        })?;
        head = head[..cut].trim_end();
    }
    let fn_name_start = head
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map(|(i, _)| i)?;
    if !head[..fn_name_start].trim_end().ends_with("fn") {
        return None;
    }
    let (_, list_open, list_close) = param_span(text, fn_name_start)?;
    if list_open != open + 1 || at > list_close {
        return None;
    }
    let (_, declared) = parse_declared(&text[list_open..list_close]);
    declared.iter().find(|d| d.name == name)?;
    let kept = declared
        .iter()
        .filter(|d| d.name != name)
        .map(|d| d.name.clone())
        .collect();
    Some((fn_name_start, name, kept))
}

/// The receiver (`&self` and friends, kept verbatim) and the parameters of a parameter list.
pub fn parse_declared(list: &str) -> (Option<String>, Vec<Declared>) {
    let mut receiver = None;
    let mut params = Vec::new();
    for raw in split_params(list) {
        let head = raw.trim_start_matches(['&', ' ']).trim_start();
        let head = head.strip_prefix("mut ").unwrap_or(head).trim_start();
        let is_receiver = head == "self"
            || head.starts_with("self:")
            || head.starts_with("self ")
            || head.starts_with('\'') && head.contains("self");
        if is_receiver && receiver.is_none() && params.is_empty() {
            receiver = Some(raw);
            continue;
        }
        let name = match split_at_top_level(&raw, ':') {
            Some((name, _)) => name.trim().trim_start_matches("mut ").trim().to_string(),
            None => raw.trim().to_string(),
        };
        params.push(Declared { raw, name });
    }
    (receiver, params)
}
