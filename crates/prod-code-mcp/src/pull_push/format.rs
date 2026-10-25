/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::pull_push::types::{ClassDecl, MemberDecl};

pub use super::modifiers::strip_override_modifiers;

/// Adjust indentation of a multi-line member text to match `target_indent`.
pub fn adjust_indentation(text: &str, target_indent: &str) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return String::new();
    }

    // Determine the base indentation of the first non-empty line
    let first_non_empty = lines.iter().find(|l| !l.trim().is_empty());
    let source_base_indent_len = first_non_empty
        .map(|l| l.len() - l.trim_start().len())
        .unwrap_or(0);

    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        if line.trim().is_empty() {
            result.push(String::new());
            continue;
        }
        let cur_indent_len = line.len() - line.trim_start().len();
        let rel_indent_len = cur_indent_len.saturating_sub(source_base_indent_len);
        let extra_spaces = " ".repeat(rel_indent_len);
        result.push(format!(
            "{target_indent}{extra_spaces}{}",
            line.trim_start()
        ));
    }
    result.join("\n")
}

pub(crate) fn normalized_member_text(member: &MemberDecl, language: &str) -> String {
    // Only the indentation before the declaration is proven trivia here. Continuation
    // indentation, trailing spaces and CRLF can all be literal data. Prefer retaining
    // a differently formatted sibling over claiming its implementation is equivalent.
    strip_override_modifiers(&member.full_text, language)
        .trim_start_matches([' ', '\t'])
        .to_owned()
}

/// Relocate indentation only where the language's lexical mask proves it is code.
/// Brace languages without a suitable literal mask need no indentation rewrite.
pub(crate) fn adjust_pulled_member(text: &str, target_indent: &str, language: &str) -> String {
    use crate::parameter_object::Language;
    let lang = match language {
        "python" => Language::Python,
        "typescript" => Language::TypeScript,
        "javascript" => Language::JavaScript,
        "swift" => Language::Swift,
        _ => return text.to_owned(),
    };
    let first = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let source_indent = &first[..first.len() - first.trim_start_matches([' ', '\t']).len()];
    if source_indent == target_indent {
        return text.to_owned();
    }
    let code = crate::caller_migration::lexical_code_mask(text, lang);
    let mut offset = 0;
    let mut result = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        if code.get(offset) == Some(&true)
            && line.starts_with(source_indent)
            && code[offset..offset + source_indent.len()]
                .iter()
                .all(|b| *b)
            && !line.trim().is_empty()
        {
            result.push_str(target_indent);
            result.push_str(&line[source_indent.len()..]);
        } else {
            result.push_str(line);
        }
        offset += line.len();
    }
    result
}

pub(crate) fn replace_python_pass(text: &mut String, class: &ClassDecl, replacement: &str) -> bool {
    let body = &text[class.body_start..class.body_end];
    let Some(pass_at) = body.find("pass") else {
        return false;
    };
    let line_start = body[..pass_at].rfind('\n').map_or(0, |i| i + 1);
    if !body[line_start..pass_at].trim().is_empty() {
        return false;
    }
    let line_end = body[pass_at..]
        .find('\n')
        .map_or(body.len(), |offset| pass_at + offset);
    text.replace_range(
        class.body_start + line_start..class.body_start + line_end,
        replacement,
    );
    true
}

pub(crate) fn cpp_member_access(
    class_text: &str,
    class: &ClassDecl,
    member: &MemberDecl,
) -> &'static str {
    let is_struct = class_text[class.decl_start..].starts_with("struct ");
    let mut access = if is_struct { "public" } else { "private" };
    let body_before = &class_text[class.body_start..member.start_offset.min(class.body_end)];
    for line in body_before.lines() {
        match line.trim() {
            "public:" => access = "public",
            "protected:" => access = "protected",
            "private:" => access = "private",
            _ => {}
        }
    }
    access
}
