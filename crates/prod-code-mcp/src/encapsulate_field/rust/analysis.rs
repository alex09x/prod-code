/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use super::super::case::is_ident;
use super::super::types::{Access, COPY, FieldDecl};

/// The field declared at `offset`, which may be anywhere in its name.
pub fn field_at(text: &str, offset: usize) -> Result<FieldDecl> {
    let offset = offset.min(text.len());
    let start = text[..offset]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(offset, |(i, _)| i);
    let end = text[offset..]
        .char_indices()
        .find(|(_, c)| !is_ident(*c))
        .map_or(text.len(), |(i, _)| offset + i);
    let name = &text[start..end];
    anyhow::ensure!(!name.is_empty(), "there is no name at this position");
    let after = text[end..].trim_start();
    anyhow::ensure!(
        after.starts_with(':') && !after.starts_with("::"),
        "`{name}` here is not a field declaration: a field is `name: Type` inside a struct"
    );
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let prefix = &text[line_start..start];
    let vis_at = line_start + (prefix.len() - prefix.trim_start().len());
    let vis = &text[vis_at..start];
    let vis_word = vis.trim_end();
    anyhow::ensure!(
        vis_word.is_empty() || vis_word == "pub" || vis_word.starts_with("pub("),
        "`{name}` here is not a field declaration: `{}` comes before it",
        vis_word
    );
    // The type runs to the comma that ends the field, or the brace that ends the struct.
    let ty_from = text.len() - after.len() + 1;
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut i = ty_from;
    while i < bytes.len() {
        match bytes[i] {
            b'-' if bytes.get(i + 1) == Some(&b'>') => i += 1,
            b'/' if bytes.get(i + 1) == Some(&b'/') && depth == 0 => break,
            b'<' | b'(' | b'[' | b'{' => depth += 1,
            b'>' | b')' | b']' | b'}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            b',' if depth == 0 => break,
            _ => {}
        }
        i += 1;
    }
    let ty = text[ty_from..i.min(text.len())].trim().to_string();
    anyhow::ensure!(!ty.is_empty(), "`{name}` has no type after its colon");
    Ok(FieldDecl {
        name: name.to_string(),
        name_at: start,
        vis: vis.to_string(),
        vis_at,
        ty,
    })
}

/// The struct whose braces contain `offset`: its name, where `struct` is, and its closing brace.
pub fn owner_at(text: &str, offset: usize) -> Option<(String, usize, usize)> {
    let mut best: Option<(String, usize, usize)> = None;
    for (at, _) in text[..offset.min(text.len())].match_indices("struct ") {
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let rest = &text[at + "struct ".len()..];
        let name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
        if name.is_empty() {
            continue;
        }
        let Some(open) = text[at..].find(['{', ';', '(']).map(|i| at + i) else {
            continue;
        };
        if text.as_bytes()[open] != b'{' {
            continue;
        }
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        if open < offset && offset < close {
            best = Some((name, at, close));
        }
    }
    best
}

/// Whether the struct at `struct_at` takes type or lifetime parameters.
pub fn is_generic(text: &str, struct_at: usize, owner: &str) -> bool {
    text[struct_at..]
        .strip_prefix("struct ")
        .and_then(|r| r.strip_prefix(owner))
        .is_some_and(|r| r.trim_start().starts_with('<'))
}

/// The opening brace of the first inherent `impl` of `owner` in `text` (not a trait impl).
pub fn inherent_impl(text: &str, owner: &str) -> Option<usize> {
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let here = offset;
        offset += line.len();
        let trimmed = line.trim_start();
        let Some(mut rest) = trimmed.strip_prefix("impl") else {
            continue;
        };
        if rest.starts_with('<') {
            let mut depth = 0i32;
            let mut end = rest.len();
            for (i, c) in rest.char_indices() {
                match c {
                    '<' => depth += 1,
                    '>' => {
                        depth -= 1;
                        if depth == 0 {
                            end = i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            rest = &rest[end..];
        } else if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let rest = rest.trim_start();
        let Some(after) = rest.strip_prefix(owner) else {
            continue;
        };
        if after.starts_with(is_ident) {
            continue;
        }
        let header_end = text[here..].find('{').map(|i| here + i)?;
        if text[here..header_end].contains(" for ") {
            continue;
        }
        return Some(header_end);
    }
    None
}

/// Whether a getter for `ty` should return the value rather than a reference: the primitive
/// `Copy` types, shared references, and an `Option` of either.
pub fn returns_by_value(ty: &str) -> bool {
    let ty = ty.trim();
    if let Some(inner) = ty.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
        return returns_by_value(inner);
    }
    COPY.contains(&ty) || (ty.starts_with('&') && !ty.starts_with("&mut"))
}

/// What the reference to a field of length `len` at `at` does.
pub fn access_at(text: &str, at: usize, len: usize) -> Access {
    let before = text[..at].trim_end();
    if !before.ends_with('.') || before.ends_with("..") {
        return Access::Blocked("a struct literal or pattern names the field");
    }
    let rest = text[at + len..].trim_start();
    if rest.starts_with('(') {
        return Access::NotAccess;
    }
    for op in ["<<=", ">>=", "+=", "-=", "*=", "/=", "%=", "|=", "&=", "^="] {
        if rest.starts_with(op) {
            return Access::Blocked("a compound assignment needs both the getter and the setter");
        }
    }
    if rest.starts_with('=') && !rest.starts_with("==") && !rest.starts_with("=>") {
        let rhs_start = text.len() - rest.len() + 1;
        return Access::Write {
            rhs: (rhs_start, expression_end(text, rhs_start)),
        };
    }
    let start = chain_start(text, before.len() - 1);
    let lead = text[..start].trim_end();
    if lead.ends_with("&mut")
        && !lead[..lead.len() - 4]
            .chars()
            .next_back()
            .is_some_and(is_ident)
    {
        return Access::Blocked("a mutable borrow of the field");
    }
    Access::Read {
        chained: rest.starts_with('.') || rest.starts_with('['),
    }
}

/// Where the expression starting at `from` ends: the `;`, `,` or closing bracket that is not
/// inside it.
fn expression_end(text: &str, from: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => {
                match crate::parameter_object::matching_bracket(text, i) {
                    Some(close) => i = close + 1,
                    None => return bytes.len(),
                }
                continue;
            }
            b'"' => {
                let mut j = i + 1;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += if bytes[j] == b'\\' { 2 } else { 1 };
                }
                i = j + 1;
                continue;
            }
            b'\'' if bytes.get(i + 2) == Some(&b'\'') => {
                i += 3;
                continue;
            }
            b';' | b',' | b')' | b']' | b'}' => return i,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// Where the receiver of the `.` at `dot` starts: `a.b().c[0]` for the dot before a field.
pub(crate) fn chain_start(text: &str, dot: usize) -> usize {
    let bytes = text.as_bytes();
    let mut i = dot;
    loop {
        while i > 0 && (bytes[i - 1] as char).is_whitespace() {
            i -= 1;
        }
        if i > 0 && matches!(bytes[i - 1], b')' | b']') {
            let mut depth = 0i32;
            let mut j = i;
            while j > 0 {
                j -= 1;
                match bytes[j] {
                    b')' | b']' => depth += 1,
                    b'(' | b'[' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            i = j;
            continue;
        }
        if i > 0 && bytes[i - 1] == b'?' {
            i -= 1;
            continue;
        }
        let ident_end = i;
        while i > 0 && is_ident(bytes[i - 1] as char) {
            i -= 1;
        }
        if i == ident_end {
            return i;
        }
        if i > 0 && bytes[i - 1] == b'.' && !(i > 1 && bytes[i - 2] == b'.') {
            i -= 1;
            continue;
        }
        if i > 1 && &text[i - 2..i] == "::" {
            i -= 2;
            continue;
        }
        return i;
    }
}
