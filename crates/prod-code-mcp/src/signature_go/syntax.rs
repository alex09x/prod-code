/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::hazards::{is_func_literal, is_literal};
use crate::signature_go::text::{is_ident_byte, skip_opaque, strip_comments};
use crate::signature_go::types::GoParam;

/// An expression or a type without comments, and with whitespace only where it separates two
/// words: what two spellings of the same code have in common.
pub(crate) fn canonical(text: &str) -> String {
    let text = strip_comments(text);
    let s = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut space) = (0usize, false);
    while i < s.len() {
        if let Some(end) = skip_opaque(s, i) {
            out.push_str(&text[i..end]);
            i = end;
            space = false;
            continue;
        }
        if s[i].is_ascii_whitespace() {
            space = true;
            i += 1;
            continue;
        }
        let ch = text[i..].chars().next().unwrap_or(' ');
        if space && is_ident_byte(s[i]) && out.as_bytes().last().is_some_and(|b| is_ident_byte(*b))
        {
            out.push(' ');
        }
        space = false;
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// A parameter list on one line, for the report.
pub(crate) fn normalize(list: &str) -> String {
    strip_comments(list)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(',')
        .to_string()
}

pub(crate) fn list_text(params: &[GoParam]) -> String {
    params
        .iter()
        .map(|p| format!("{} {}", p.name, p.ty))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn suffix(results: &str) -> String {
    if results.is_empty() {
        String::new()
    } else {
        format!(" {results}")
    }
}

pub(crate) fn go_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
        && !matches!(
            name,
            "break"
                | "default"
                | "func"
                | "interface"
                | "select"
                | "case"
                | "defer"
                | "go"
                | "map"
                | "struct"
                | "chan"
                | "else"
                | "goto"
                | "package"
                | "switch"
                | "const"
                | "fallthrough"
                | "if"
                | "range"
                | "type"
                | "continue"
                | "for"
                | "import"
                | "return"
                | "var"
        )
        && name != "_"
}

pub(crate) fn primitive_type(ty: &str) -> bool {
    matches!(
        ty,
        "string"
            | "byte"
            | "rune"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "uintptr"
            | "float32"
            | "float64"
            | "complex64"
            | "complex128"
    )
}

pub(crate) fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

pub(crate) fn scalar_literal(value: &str) -> bool {
    if value.is_empty() || !is_literal(value) || is_func_literal(value) {
        return false;
    }
    match value.as_bytes()[0] {
        quote @ (b'"' | b'\'' | b'`') => {
            value.len() >= 2 && value.as_bytes().last() == Some(&quote)
        }
        _ => true,
    }
}
