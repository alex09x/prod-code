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
use crate::pull_push::types::{ClassDecl, MemberDecl, MemberKind};
use std::path::Path;

pub fn parse_swift_classes(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some(pos) = text[idx..].find("class ") {
            let class_kw_pos = idx + pos;
            if class_kw_pos > 0 && bytes[class_kw_pos - 1].is_ascii_alphanumeric() {
                idx = class_kw_pos + 6;
                continue;
            }
            let after_kw = &text[class_kw_pos + 6..];
            let open_brace_rel = match after_kw.find('{') {
                Some(p) => p,
                None => {
                    idx = class_kw_pos + 6;
                    continue;
                }
            };
            let header = after_kw[..open_brace_rel].trim();
            let open_brace_pos = class_kw_pos + 6 + open_brace_rel;

            let close_brace_pos = match find_matching_brace(text, open_brace_pos) {
                Some(p) => p,
                None => {
                    idx = class_kw_pos + 6;
                    continue;
                }
            };

            let (class_name, super_names) = if let Some(colon_idx) = header.find(':') {
                let name = header[..colon_idx].trim().to_string();
                let bases_part = &header[colon_idx + 1..];
                let bases: Vec<String> = bases_part
                    .split(',')
                    .map(|b| b.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (name, bases)
            } else {
                (
                    header.split_whitespace().next().unwrap_or("").to_string(),
                    Vec::new(),
                )
            };

            let decl_start = class_kw_pos;
            let decl_end = close_brace_pos + 1;
            let body_start = open_brace_pos + 1;
            let body_end = close_brace_pos;

            let body_text = &text[body_start..body_end];
            let members = parse_swift_members(body_text, body_start);

            classes.push(ClassDecl {
                name: class_name,
                language: "swift".to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent: "    ".to_string(),
                members,
            });

            idx = close_brace_pos + 1;
        } else {
            break;
        }
    }

    classes
}

fn parse_swift_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
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
        if trimmed.is_empty() || trimmed.starts_with("//") {
            i += 1;
            continue;
        }

        let is_override = trimmed.contains("override ");

        // func
        if let Some(func_idx) = trimmed.find("func ") {
            let after_func = trimmed[func_idx + 5..].trim_start();
            if let Some(open_paren) = after_func.find('(') {
                let ident = after_func[..open_paren].trim();
                let member_start = line_offsets[i];
                if let Some(open_brace_rel) = body[line_offsets[i] - body_offset..].find('{') {
                    let global_open = line_offsets[i] + open_brace_rel;
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
                }
            }
        } else if trimmed.starts_with("var ")
            || trimmed.starts_with("let ")
            || trimmed.contains(" var ")
            || trimmed.contains(" let ")
        {
            // property
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if let Some(kw_pos) = words.iter().position(|&w| w == "var" || w == "let")
                && let Some(ident_raw) = words.get(kw_pos + 1)
            {
                let ident = ident_raw.trim_end_matches(':').trim();
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
