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

/// What would make the old value fit the new type, when the error says so plainly.
///
/// Only the two shapes that are unambiguous: the new type meeting the old one, either way
/// round. Anything else gets no suggestion rather than a guess.
pub fn suggest(message: &str, was: &str, now: &str) -> Option<String> {
    let (expected, found) = parse_mismatch(message)?;
    let (expected, found) = (type_name(&expected), type_name(&found));
    let (was, now) = (type_name(was), type_name(now));
    if expected == now && found == was {
        return Some(format!(
            "the value here is still `{was}`; convert it to `{now}`"
        ));
    }
    if expected == was && found == now {
        return Some(format!(
            "this place still wants `{was}` and is now given `{now}`; migrate it too, or convert \
             back here"
        ));
    }
    None
}

/// A type as the analyzer names it. It names a type as it is in scope, not as the declaration
/// spells it — `std::time::Duration` comes back as `Duration`, at any depth — and it spells out
/// the default allocator: `Box<str>` comes back as `Box<str, Global>`.
pub fn type_name(ty: &str) -> String {
    let mut out = String::new();
    for c in ty.trim().replace(", Global>", ">").chars() {
        out.push(c);
        if out.ends_with("::") {
            out.truncate(out.len() - 2);
            while out.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                out.pop();
            }
        }
    }
    out
}

/// `expected X, found Y` out of an analyzer message across Rust and polyglot languages.
pub(crate) fn parse_mismatch(message: &str) -> Option<(String, String)> {
    // Rust: "expected X, found Y"
    if let Some(rest) = message.split("expected ").nth(1)
        && let Some((expected, rest)) = rest.split_once(", found ")
    {
        let found = rest
            .split(['\n', ' '])
            .next()
            .unwrap_or(rest)
            .trim_end_matches(['.', ',']);
        return Some((expected.trim().to_string(), found.trim().to_string()));
    }
    // TypeScript: "Type 'X' is not assignable to type 'Y'"
    if let Some((before, after)) = message.split_once(" is not assignable to type ") {
        let found = before
            .rsplit('\'')
            .nth(1)
            .or_else(|| before.split('\'').nth(1))
            .unwrap_or(before)
            .trim();
        let expected = after.split('\'').nth(1).unwrap_or(after).trim();
        return Some((expected.to_string(), found.to_string()));
    }
    // Python (basedpyright): 'Expression of type "X" cannot be assigned to declared type "Y"'
    if message.contains("cannot be assigned to") {
        let parts: Vec<&str> = message.split('"').collect();
        if parts.len() >= 4 {
            let found = parts[1];
            let expected = parts[parts.len() - 2];
            return Some((expected.to_string(), found.to_string()));
        }
    }
    // Go: "cannot use X (variable of type A) as B value"
    if message.contains("cannot use")
        && message.contains(" as ")
        && let Some(of_type) = message.split("variable of type ").nth(1)
        && let Some((found, rest)) = of_type.split_once(')')
        && let Some(as_type) = rest.split(" as ").nth(1)
    {
        let expected = as_type.split_whitespace().next().unwrap_or(as_type).trim();
        return Some((expected.to_string(), found.trim().to_string()));
    }
    // Swift: "cannot convert value of type 'X' to specified type 'Y'"
    if message.contains("cannot convert value of type") {
        let parts: Vec<&str> = message.split('\'').collect();
        if parts.len() >= 4 {
            let found = parts[1];
            let expected = parts[parts.len() - 2];
            return Some((expected.to_string(), found.to_string()));
        }
    }
    // C++: "no viable conversion from 'X' to 'Y'"
    if message.contains("no viable conversion from") {
        let parts: Vec<&str> = message.split('\'').collect();
        if parts.len() >= 4 {
            let found = parts[1];
            let expected = parts[3];
            return Some((expected.to_string(), found.to_string()));
        }
    }
    None
}

/// Generates a language-idiomatic conversion expression when `convert: true`.
pub fn language_conversion(expr: &str, target_type: &str, lang: Language) -> String {
    let t = target_type.trim();
    match lang {
        Language::Rust => into_call(expr),
        Language::TypeScript | Language::JavaScript => {
            if t == "number" {
                format!("Number({expr})")
            } else if t == "string" {
                format!("String({expr})")
            } else if t == "boolean" {
                format!("Boolean({expr})")
            } else if t == "bigint" {
                format!("BigInt({expr})")
            } else if expr
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
            {
                format!("{t}({expr})")
            } else {
                format!("({expr} as {t})")
            }
        }
        Language::Python => {
            format!("{t}({expr})")
        }
        Language::Go => {
            format!("{t}({expr})")
        }
        Language::Swift => {
            format!("{t}({expr})")
        }
        Language::Cpp | Language::C => {
            format!("static_cast<{t}>({expr})")
        }
        Language::Java => {
            if matches!(
                t,
                "int" | "long" | "float" | "double" | "byte" | "short" | "char"
            ) {
                format!("({t}) ({expr})")
            } else if t == "String" {
                format!("String.valueOf({expr})")
            } else {
                format!("({t}) ({expr})")
            }
        }
    }
}

/// `expr.into()`, with parentheses unless the expression is a path, a call chain or a literal
/// that a method call binds to as a whole.
pub fn into_call(expr: &str) -> String {
    let mut depth = 0i32;
    let mut simple = !expr.is_empty();
    for c in expr.chars() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth > 0 => {}
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == ':' => {}
            _ => simple = false,
        }
    }
    let starts_well = expr
        .chars()
        .next()
        .is_some_and(|c| c.is_alphanumeric() || c == '_');
    if simple && starts_well {
        format!("{expr}.into()")
    } else {
        format!("({expr}).into()")
    }
}
