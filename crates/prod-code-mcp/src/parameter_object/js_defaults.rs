/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, ensure};

use super::type_render::js_key;
use super::types::{Kind, Language, Param};

pub(super) fn preserve_callee_defaults(
    language: Language,
    declared: &[Param],
    bundled: &[usize],
) -> bool {
    matches!(language, Language::JavaScript | Language::TypeScript)
        && bundled
            .iter()
            .any(|index| declared[*index].default.is_some())
}

pub(super) fn type_accepting_undefined(ty: &str) -> String {
    if ty.split('|').any(|part| part.trim() == "undefined") {
        ty.to_string()
    } else {
        format!("{ty} | undefined")
    }
}

pub(super) fn ensure_contiguous_bundle(
    callee: &str,
    declared: &[Param],
    bundled: &[usize],
) -> Result<()> {
    let Some((&first, &last)) = bundled.first().zip(bundled.last()) else {
        return Ok(());
    };
    for index in first..=last {
        ensure!(
            declared[index].kind == Kind::Marker || bundled.contains(&index),
            "`{callee}` has a non-bundled parameter between JavaScript/TypeScript defaults; \
             bundling would change the order or scope in which those defaults are evaluated"
        );
    }
    Ok(())
}

pub(super) fn parameter_pattern(
    language: Language,
    name: &str,
    declared: &[Param],
    bundled: &[usize],
) -> String {
    let fields = bundled
        .iter()
        .map(|index| {
            let param = &declared[*index];
            match &param.default {
                Some(default) => format!("{} = {default}", param.name),
                None => param.name.clone(),
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    match language {
        Language::TypeScript => format!("{{ {fields} }}: {name}"),
        Language::JavaScript => format!("{{ {fields} }}"),
        _ => unreachable!("only JavaScript and TypeScript defaults use parameter patterns"),
    }
}

pub(super) fn binding_declaration(
    language: Language,
    binding: &str,
    name: &str,
    declared: &[Param],
    bundled: &[usize],
    indent: &str,
) -> String {
    let fields = bundled
        .iter()
        .map(|index| {
            let field = &declared[*index].name;
            format!("{}: {field}", js_key(field))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let annotation = if language == Language::TypeScript {
        format!(": {name}")
    } else {
        String::new()
    };
    format!("const {binding}{annotation} = {{ {fields} }};\n{indent}")
}

pub(super) fn indentation_at(source: &str, at: usize) -> String {
    let line_start = source[..at].rfind('\n').map_or(0, |index| index + 1);
    source[line_start..at]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect()
}

/// Find the first executable statement after a function's directive prologue.
pub(super) fn after_directives(source: &str, start: usize, end: usize) -> usize {
    let mut at = start;
    loop {
        let (token, _) = skip_trivia(source, at, end);
        if token >= end {
            return token;
        }
        let Some(literal_end) = string_literal_end(source, token, end) else {
            return token;
        };
        let (next, line_break) = skip_trivia(source, literal_end, end);
        if next < end && source.as_bytes()[next] == b';' {
            at = next + 1;
            continue;
        }
        if next == end {
            return next;
        }
        if line_break && !continues_expression(source, next, end) {
            at = next;
            continue;
        }
        return token;
    }
}

fn skip_trivia(source: &str, mut at: usize, end: usize) -> (usize, bool) {
    let bytes = source.as_bytes();
    let mut line_break = false;
    while at < end {
        if bytes[at].is_ascii_whitespace() {
            line_break |= matches!(bytes[at], b'\n' | b'\r');
            at += 1;
        } else if bytes.get(at..at + 2) == Some(b"//") {
            at += 2;
            while at < end && !matches!(bytes[at], b'\n' | b'\r') {
                at += 1;
            }
        } else if bytes.get(at..at + 2) == Some(b"/*") {
            at += 2;
            while at + 1 < end && bytes.get(at..at + 2) != Some(b"*/") {
                line_break |= matches!(bytes[at], b'\n' | b'\r');
                at += 1;
            }
            at = (at + 2).min(end);
        } else {
            break;
        }
    }
    (at, line_break)
}

fn string_literal_end(source: &str, start: usize, end: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let quote = *bytes.get(start)?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let mut at = start + 1;
    while at < end {
        match bytes[at] {
            byte if byte == quote => return Some(at + 1),
            b'\\' => at += 2,
            b'\n' | b'\r' => return None,
            _ => at += 1,
        }
    }
    None
}

fn continues_expression(source: &str, at: usize, end: usize) -> bool {
    let byte = source.as_bytes()[at];
    if matches!(
        byte,
        b'(' | b'['
            | b'.'
            | b'+'
            | b'-'
            | b'/'
            | b'*'
            | b'%'
            | b'?'
            | b'`'
            | b'&'
            | b'|'
            | b'^'
            | b'<'
            | b'>'
            | b'='
            | b','
    ) {
        return true;
    }
    let tail = &source[at..end];
    ["in", "instanceof", "as", "satisfies"].iter().any(|word| {
        tail.starts_with(word)
            && !tail
                .as_bytes()
                .get(word.len())
                .is_some_and(u8::is_ascii_alphanumeric)
    })
}

#[cfg(test)]
mod tests {
    use super::{after_directives, type_accepting_undefined};

    #[test]
    fn binding_insertion_follows_directives_and_leading_comments() {
        let source = "{\n  // leading\n  \"use strict\";\n  'use asm'\n  return value;\n}";
        let start = source.find('{').unwrap() + 1;
        let end = source.rfind('}').unwrap();
        let insertion = after_directives(source, start, end);
        assert_eq!(&source[insertion..end], "return value;\n");
    }

    #[test]
    fn defaulted_types_accept_explicit_undefined_without_duplicate_union_members() {
        assert_eq!(type_accepting_undefined("string"), "string | undefined");
        assert_eq!(
            type_accepting_undefined("string | undefined"),
            "string | undefined"
        );
    }
}
