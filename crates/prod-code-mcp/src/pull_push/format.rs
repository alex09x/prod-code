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

/// Strip `override` or `final` modifiers from member text when pulling up to base.
pub fn strip_override_modifiers(text: &str, language: &str) -> String {
    match language {
        "typescript" | "javascript" | "swift" => {
            let lines: Vec<&str> = text.split('\n').collect();
            let mut out = Vec::with_capacity(lines.len());
            for line in lines {
                if let Some(pos) = line.find("override ") {
                    let cleaned = format!("{}{}", &line[..pos], &line[pos + 9..]);
                    out.push(cleaned);
                } else if let Some(pos) = line.find("override\t") {
                    let cleaned = format!("{}{}", &line[..pos], &line[pos + 9..]);
                    out.push(cleaned);
                } else {
                    out.push(line.to_string());
                }
            }
            out.join("\n")
        }
        "cpp" => {
            let mut s = text.to_string();
            if let Some(pos) = s.find(" override") {
                s.replace_range(pos..pos + 9, "");
            }
            if let Some(pos) = s.find(" final") {
                s.replace_range(pos..pos + 6, "");
            }
            s
        }
        _ => text.to_string(),
    }
}

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
    let text = strip_override_modifiers(&member.full_text, language).replace("\r\n", "\n");
    let lines = text.lines().collect::<Vec<_>>();
    let common_indent = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|c| *c == ' ' || *c == '\t').count())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| {
            let cut = if line.trim().is_empty() || common_indent == 0 {
                0
            } else {
                line.char_indices()
                    .take_while(|(_, c)| *c == ' ' || *c == '\t')
                    .nth(common_indent - 1)
                    .map_or(0, |(byte, c)| byte + c.len_utf8())
            };
            line[cut..].trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
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
