/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::cpp_modifiers::strip_cpp_suffix;
use super::types::{ClassDecl, MemberDecl, MemberKind};
use anyhow::{Context, Result};

/// Remove only declaration modifiers whose position is unambiguous. Leave unfamiliar
/// syntax alone rather than searching bodies, defaults, comments or literals.
pub fn strip_override_modifiers(text: &str, language: &str) -> String {
    match language {
        "typescript" | "javascript" | "swift" => strip_prefix_override(text, language),
        "cpp" => strip_cpp_suffix(text).unwrap_or_else(|| text.to_owned()),
        _ => text.to_owned(),
    }
}

pub(super) fn strip_moved_member_modifiers(member: &MemberDecl, language: &str) -> Result<String> {
    anyhow::ensure!(
        member.kind != MemberKind::Opaque,
        "cannot safely identify C++ declaration modifiers for `{}`; nothing was written",
        member.name
    );
    if language == "cpp" && member.kind == MemberKind::Method {
        return strip_cpp_suffix(&member.full_text).with_context(|| {
            format!(
                "cannot safely identify C++ declaration modifiers for `{}`; nothing was written",
                member.name
            )
        });
    }
    Ok(strip_override_modifiers(&member.full_text, language))
}

/// Work on parsed declarations, never on a whole body containing literal data.
pub(crate) fn strip_class_member_modifiers(text: &str, class: &ClassDecl) -> Result<String> {
    let mut body = text[class.body_start..class.body_end].to_owned();
    for member in class.members.iter().rev() {
        // Constructors have no override modifiers and use a different suffix grammar.
        if class.language == "cpp" && member.name == class.name {
            // The parser retains an opaque remainder when it cannot locate a
            // member boundary. That is not a safely parsed constructor to skip.
            member
                .full_text
                .find('(')
                .and_then(|open| crate::parameter_object::matching_bracket(&member.full_text, open))
                .and_then(|end| {
                    super::cpp_modifiers::cpp_member_delimiter(&member.full_text, end + 1)
                })
                .context("cannot safely identify C++ constructor boundary; nothing was written")?;
            continue;
        }
        let stripped = strip_moved_member_modifiers(member, &class.language)?;
        body.replace_range(
            member.start_offset - class.body_start..member.end_offset - class.body_start,
            &stripped,
        );
    }
    Ok(body)
}

fn strip_prefix_override(text: &str, language: &str) -> String {
    let Some((_, removals)) = declaration_prefix(text, language) else {
        return text.to_owned();
    };
    let mut result = text.to_owned();
    for range in removals.into_iter().rev() {
        result.replace_range(range, "");
    }
    result
}

/// Locate Swift initializer declarations after their attributes and modifiers.
/// Attribute arguments cannot be mistaken for the initializer keyword.
pub(crate) fn swift_declaration_start(text: &str) -> Option<usize> {
    declaration_prefix(text, "swift").map(|(offset, _)| offset)
}

/// The member parser and modifier editor must cross exactly the same decorators.
pub(super) fn typescript_declaration_start(text: &str) -> Option<(usize, bool)> {
    declaration_prefix(text, "typescript").map(|(offset, removals)| (offset, !removals.is_empty()))
}

fn declaration_prefix(text: &str, language: &str) -> Option<(usize, Vec<std::ops::Range<usize>>)> {
    let mut offset = trivia_end(text, 0)?;
    let mut removals = Vec::new();
    loop {
        let rest = &text[offset..];
        if rest.starts_with('@') {
            offset = attribute_end(text, offset, language)?;
            continue;
        }
        let end = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        let word = &rest[..end];
        let tail = &rest[end..];
        if !matches!(
            word,
            "override"
                | "public"
                | "private"
                | "protected"
                | "static"
                | "abstract"
                | "declare"
                | "readonly"
                | "async"
                | "get"
                | "set"
                | "open"
                | "final"
                | "internal"
                | "fileprivate"
                | "nonisolated"
                | "required"
                | "convenience"
                | "dynamic"
                | "class"
        ) {
            break;
        }
        let next_offset = trivia_end(text, offset + end)?;
        if next_offset == offset + end {
            break;
        }
        let next = &text[next_offset..];
        // A field/method named `override` is not an override modifier.
        if !next.starts_with(|c: char| c.is_alphabetic() || c == '_') && !next.starts_with('@') {
            break;
        }
        if word == "override" {
            // Keep comments, including any literal-looking words inside them.
            let whitespace = tail.len() - tail.trim_start_matches(char::is_whitespace).len();
            removals.push(offset..offset + end + whitespace);
        }
        offset = next_offset;
    }
    Some((offset, removals))
}

/// Preserve attributes verbatim while crossing their balanced argument lists.
/// The language's lexical mask excludes parentheses in strings/comments.
fn attribute_end(text: &str, start: usize, language: &str) -> Option<usize> {
    let name = &text[start + 1..];
    if !name.starts_with(|c: char| c.is_alphabetic() || c == '_') {
        return None;
    }
    let end = name
        .find(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
        .unwrap_or(name.len());
    let args = trivia_end(text, start + 1 + end)?;
    if !text[args..].starts_with('(') {
        return Some(args);
    }
    let lang = match language {
        "swift" => crate::parameter_object::Language::Swift,
        "javascript" => crate::parameter_object::Language::JavaScript,
        _ => crate::parameter_object::Language::TypeScript,
    };
    let code = crate::caller_migration::lexical_code_mask(text, lang);
    let mut depth = 0usize;
    for (offset, byte) in text.bytes().enumerate().skip(args) {
        if !code[offset] {
            continue;
        }
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return trivia_end(text, offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Skip declaration trivia without discarding its bytes from the output.
pub(super) fn trivia_end(text: &str, mut offset: usize) -> Option<usize> {
    loop {
        let rest = &text[offset..];
        offset += rest.len() - rest.trim_start_matches(char::is_whitespace).len();
        let rest = &text[offset..];
        if rest.starts_with("//") {
            offset += rest.find('\n').unwrap_or(rest.len());
        } else if rest.starts_with("/*") {
            offset += rest.find("*/")? + 2;
        } else {
            return Some(offset);
        }
    }
}
