/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::pull_push::braces::find_matching_brace;
use crate::pull_push::cpp_modifiers::cpp_member_delimiter;
use crate::pull_push::types::{ClassDecl, MemberDecl, MemberKind};
use std::path::Path;

pub fn parse_cpp_classes(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        let next_class = text[idx..].find("class ");
        let next_struct = text[idx..].find("struct ");
        let (kw_pos, kw_len) = match (next_class, next_struct) {
            (Some(c), Some(s)) if c < s => (idx + c, 6),
            (Some(_), Some(s)) => (idx + s, 7),
            (Some(c), None) => (idx + c, 6),
            (None, Some(s)) => (idx + s, 7),
            (None, None) => break,
        };

        if kw_pos > 0 && bytes[kw_pos - 1].is_ascii_alphanumeric() {
            idx = kw_pos + kw_len;
            continue;
        }

        let after_kw = &text[kw_pos + kw_len..];
        let open_brace_rel = match after_kw.find('{') {
            Some(p) => p,
            None => {
                idx = kw_pos + kw_len;
                continue;
            }
        };

        // Forward declarations: `class Foo;` before `{`
        let header = after_kw[..open_brace_rel].trim();
        if header.contains(';') {
            idx = kw_pos + kw_len;
            continue;
        }

        let open_brace_pos = kw_pos + kw_len + open_brace_rel;
        let close_brace_pos = match find_matching_brace(text, open_brace_pos) {
            Some(p) => p,
            None => {
                idx = kw_pos + kw_len;
                continue;
            }
        };

        let (class_name, super_names) = if let Some(colon_idx) = header.find(':') {
            let name = header[..colon_idx].trim().to_string();
            let bases_part = &header[colon_idx + 1..];
            let bases: Vec<String> = bases_part
                .split(',')
                .map(|b| {
                    let parts: Vec<&str> = b.split_whitespace().collect();
                    parts.last().cloned().unwrap_or("").to_string()
                })
                .filter(|s| !s.is_empty())
                .collect();
            (name, bases)
        } else {
            (
                header.split_whitespace().next().unwrap_or("").to_string(),
                Vec::new(),
            )
        };

        if class_name.is_empty() {
            idx = close_brace_pos + 1;
            continue;
        }

        let decl_start = kw_pos;
        let decl_end = if close_brace_pos + 1 < text.len() && bytes[close_brace_pos + 1] == b';' {
            close_brace_pos + 2
        } else {
            close_brace_pos + 1
        };
        let body_start = open_brace_pos + 1;
        let body_end = close_brace_pos;

        let body_text = &text[body_start..body_end];
        let members = parse_cpp_members(body_text, body_start);

        classes.push(ClassDecl {
            name: class_name,
            language: "cpp".to_string(),
            file_path: file_path.to_path_buf(),
            super_names,
            decl_start,
            decl_end,
            body_start,
            body_end,
            indent: "    ".to_string(),
            members,
        });

        idx = decl_end;
    }

    classes
}

fn parse_cpp_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
    let mut members = Vec::new();
    let lines: Vec<&str> = body.split('\n').collect();
    let mut line_offsets = Vec::with_capacity(lines.len());
    let mut cur = body_offset;
    for l in &lines {
        line_offsets.push(cur);
        cur += l.len() + 1;
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("//")
            || trimmed.starts_with("/*")
            || trimmed == "public:"
            || trimmed == "protected:"
            || trimmed == "private:"
        {
            i += 1;
            continue;
        }

        let is_override = trimmed.contains("override") || trimmed.contains("final");

        // Method: contains `(` and either `{` or `;`
        if let Some(open_paren) = trimmed.find('(') {
            let before_paren = trimmed[..open_paren].trim();
            let ident = before_paren.split_whitespace().last().unwrap_or("");
            let name = ident.strip_prefix('~').unwrap_or(ident);
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let member_start = line_offsets[i];
                let rest = &body[member_start - body_offset..];
                let paren_offset = line.len() - line.trim_start().len() + open_paren;
                let parameters_end = crate::parameter_object::matching_bracket(rest, paren_offset);
                let delimiter = parameters_end.and_then(|end| cpp_member_delimiter(rest, end + 1));
                let body_open = delimiter.filter(|&offset| rest.as_bytes()[offset] == b'{');
                if let Some(open_brace_rel) = body_open {
                    let global_open = member_start + open_brace_rel;
                    if let Some(global_close) = find_matching_brace(body, global_open - body_offset)
                    {
                        let member_end = body_offset + global_close + 1;
                        let full_text =
                            body[member_start - body_offset..member_end - body_offset].to_string();
                        members.push(MemberDecl {
                            name: ident.to_string(),
                            kind: MemberKind::Method,
                            is_override,
                            start_offset: member_start,
                            end_offset: member_end,
                            full_text,
                        });
                        while i < lines.len() && line_offsets[i] < member_end {
                            i += 1;
                        }
                        continue;
                    }
                } else if let Some(end) =
                    delimiter.filter(|&offset| rest.as_bytes()[offset] == b';')
                {
                    // Method declaration
                    let member_end = member_start + end + 1;
                    let full_text =
                        body[member_start - body_offset..member_end - body_offset].to_string();
                    members.push(MemberDecl {
                        name: ident.to_string(),
                        kind: MemberKind::Method,
                        is_override,
                        start_offset: member_start,
                        end_offset: member_end,
                        full_text,
                    });
                    while i < lines.len() && line_offsets[i] < member_end {
                        i += 1;
                    }
                    continue;
                }

                // An unrecognized boundary is not an absent member. Retain the
                // opaque remainder so relocation/modifier editing can refuse it,
                // and do not interpret lines inside it as sibling declarations.
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::Opaque,
                    is_override,
                    start_offset: member_start,
                    end_offset: body_offset + body.len(),
                    full_text: rest.to_string(),
                });
                break;
            }
        } else if let Some(before_semi_raw) = trimmed.strip_suffix(';') {
            // Field
            let before_semi = before_semi_raw.trim();
            let before_assign = before_semi.split('=').next().unwrap_or(before_semi).trim();
            let ident = before_assign.split_whitespace().last().unwrap_or("");
            if !ident.is_empty() && ident.chars().all(|c| c.is_alphanumeric() || c == '_') {
                let member_start = line_offsets[i];
                let member_end = line_offsets[i] + line.len();
                let full_text =
                    body[member_start - body_offset..member_end - body_offset].to_string();
                members.push(MemberDecl {
                    name: ident.to_string(),
                    kind: MemberKind::Field,
                    is_override,
                    start_offset: member_start,
                    end_offset: member_end,
                    full_text,
                });
            }
        }

        i += 1;
    }

    members
}
