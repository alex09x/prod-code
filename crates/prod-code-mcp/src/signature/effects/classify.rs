/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::call_sites::blank_comments;
use crate::signature::parse::is_ident;
use crate::signature::types::{ArgKind, FIELD_DEREF, ParamFacts, REF_COERCION, UNCONFIRMED_TYPE};

/// The built-in scalar types, by the names that usually mean them.
pub const SCALARS: [&str; 16] = [
    "bool", "char", "u8", "u16", "u32", "u64", "u128", "usize", "i8", "i16", "i32", "i64", "i128",
    "isize", "f32", "f64",
];

/// What evaluating `expr`, passed for a parameter with `facts`, can do. Comments are not code:
/// they are blanked first, nested ones whole.
pub fn classify_arg(expr: &str, facts: &ParamFacts) -> ArgKind {
    let conversion = if facts.coercion_free {
        None
    } else if facts.reference {
        Some(REF_COERCION)
    } else {
        Some(UNCONFIRMED_TYPE)
    };
    match blank_comments(expr) {
        Some(code) => classify(code.trim(), conversion),
        None => ArgKind::Effectful,
    }
}

/// `conversion` is why converting the value to the parameter's type may run code, `None` when it
/// cannot.
pub fn classify(e: &str, conversion: Option<&'static str>) -> ArgKind {
    if let Some(rest) = e.strip_prefix('&') {
        let rest = rest.trim_start();
        let rest = match rest.strip_prefix("mut") {
            Some(r) if r.starts_with(char::is_whitespace) => r,
            _ => rest,
        };
        // Taking a reference runs nothing; converting it to the parameter's type may.
        return match (classify(rest.trim(), None), conversion) {
            (ArgKind::Place, Some(why)) => ArgKind::Unproven(why),
            (kind, _) => kind,
        };
    }
    // A cast is between built-in types, and what it makes is not converted by a `Deref`.
    if let Some((value, ty)) = e.rsplit_once(" as ")
        && is_path(ty.trim())
    {
        return classify(value.trim(), None);
    }
    if is_literal(e) {
        ArgKind::Literal
    } else if is_path(e) {
        conversion.map_or(ArgKind::Place, ArgKind::Unproven)
    } else if is_place(e) {
        ArgKind::Unproven(FIELD_DEREF)
    } else {
        ArgKind::Effectful
    }
}

pub fn is_literal(e: &str) -> bool {
    if matches!(e, "true" | "false" | "()") {
        return true;
    }
    let number = e.strip_prefix('-').unwrap_or(e);
    if number.starts_with(|c: char| c.is_ascii_digit()) {
        return !number.contains("..")
            && number
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    }
    is_string_literal(e) || is_char_literal(e)
}

pub fn is_string_literal(e: &str) -> bool {
    let Some(quote) = e.find('"') else {
        return false;
    };
    let prefix = &e[..quote];
    let body = &e[quote + 1..];
    if matches!(prefix, "" | "b" | "c") {
        let mut escaped = false;
        for (i, c) in body.char_indices() {
            match c {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => return i + 1 == body.len(),
                _ => {}
            }
        }
        return false;
    }
    let raw = prefix.strip_prefix(['b', 'c']).unwrap_or(prefix);
    let Some(hashes) = raw.strip_prefix('r') else {
        return false;
    };
    if !hashes.chars().all(|c| c == '#') {
        return false;
    }
    let close = format!("\"{hashes}");
    body.find(&close) == Some(body.len().wrapping_sub(close.len()))
}

pub fn is_char_literal(e: &str) -> bool {
    let inner = e
        .strip_prefix("b'")
        .or_else(|| e.strip_prefix('\''))
        .and_then(|r| r.strip_suffix('\''));
    match inner {
        Some("\\'") => true,
        Some(i) if i.starts_with('\\') => i.len() > 1 && !i[1..].contains('\''),
        Some(i) => i.chars().count() == 1,
        None => false,
    }
}

/// `x`, `a::B`, `self.field.0`: a path, then fields (`.await` is not one).
pub fn is_place(e: &str) -> bool {
    let mut parts = e.split('.');
    parts.next().is_some_and(is_path)
        && parts.all(|p| {
            (is_ident(p) && p != "await")
                || (!p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        })
}

pub fn is_path(e: &str) -> bool {
    let e = e.strip_prefix("::").unwrap_or(e);
    !e.is_empty() && e.split("::").all(is_ident)
}

/// Types written with syntax rather than a name, which no declaration can shadow and which are
/// dropped without running code: references, raw and function pointers.
pub fn is_pointer(ty: &str) -> bool {
    ty.starts_with('&')
        || ty.starts_with("*const ")
        || ty.starts_with("*mut ")
        || ty.starts_with("fn(")
        || ty.starts_with("fn (")
        || ty.starts_with("unsafe fn")
        || ty.starts_with("unsafe extern ")
        || ty.starts_with("extern ")
}
