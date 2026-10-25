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
use crate::pull_push::modifiers::typescript_declaration_start;
use crate::pull_push::types::{ClassDecl, MemberDecl, MemberKind};
use std::path::Path;

pub fn parse_ts_classes(text: &str, file_path: &Path, language: &str) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let bytes = text.as_bytes();
    let mut idx = 0;

    while idx < bytes.len() {
        if let Some(pos) = text[idx..].find("class ") {
            let class_kw_pos = idx + pos;
            // Ensure "class" is a whole keyword
            if class_kw_pos > 0 && text.as_bytes()[class_kw_pos - 1].is_ascii_alphanumeric() {
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

            // Parse class name and superclass from header: e.g. "Dog extends Animal"
            let tokens: Vec<&str> = header.split_whitespace().collect();
            if tokens.is_empty() {
                idx = close_brace_pos + 1;
                continue;
            }
            let class_name = tokens[0]
                .split('<')
                .next()
                .unwrap_or(tokens[0])
                .trim()
                .to_string();
            let mut super_names = Vec::new();
            if let Some(ext_pos) = tokens.iter().position(|&t| t == "extends")
                && let Some(base) = tokens.get(ext_pos + 1)
            {
                let base_clean = base.split('<').next().unwrap_or(base).trim();
                super_names.push(base_clean.to_string());
            }

            // Find decl_start (handle optional `export `, `abstract `)
            let mut line_start = class_kw_pos;
            while line_start > 0 && bytes[line_start - 1] != b'\n' {
                line_start -= 1;
            }
            let decl_start = line_start;
            let decl_end = close_brace_pos + 1;
            let body_start = open_brace_pos + 1;
            let body_end = close_brace_pos;

            // Default indent
            let indent = "    ".to_string();

            // Parse members inside body
            let body_text = &text[body_start..body_end];
            let members = parse_ts_members(body_text, body_start);

            classes.push(ClassDecl {
                name: class_name,
                language: language.to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent,
                members,
            });

            idx = close_brace_pos + 1;
        } else {
            break;
        }
    }

    classes
}

fn parse_ts_members(body: &str, body_offset: usize) -> Vec<MemberDecl> {
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
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with("/*") {
            i += 1;
            continue;
        }

        let member_start = line_offsets[i];
        let rest = &body[member_start - body_offset..];
        let Some((declaration_start, is_override)) = typescript_declaration_start(rest) else {
            i += 1;
            continue;
        };
        let declaration = &rest[declaration_start..];
        let declaration_line = declaration.lines().next().unwrap_or("").trim_end();

        // Check for method: identifier followed by `(` and containing `{`
        if let Some(open_paren) = declaration_line.find('(') {
            let before_paren = declaration_line[..open_paren].trim();
            let ident = before_paren.split_whitespace().last().unwrap_or("");
            if !ident.is_empty()
                && ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                && ident != "if"
                && ident != "while"
                && ident != "for"
            {
                // Find matching brace for method body
                let parameters_end =
                    typescript_parameters_end(rest, declaration_start + open_paren);
                if let Some(parameters_end) = parameters_end
                    && let Some(open_brace_in_body) = rest[parameters_end + 1..].find('{')
                {
                    let global_open = member_start + parameters_end + 1 + open_brace_in_body;
                    if let Some(global_close) = find_matching_brace(body, global_open - body_offset)
                    {
                        let member_end = body_offset + global_close + 1;
                        let full_text =
                            body[member_start - body_offset..member_end - body_offset].to_string();
                        if ident != "constructor" {
                            members.push(MemberDecl {
                                name: ident.to_string(),
                                kind: MemberKind::Method,
                                is_override,
                                start_offset: member_start,
                                end_offset: member_end,
                                full_text,
                            });
                        }
                        // Skip past method
                        while i < lines.len() && line_offsets[i] < member_end {
                            i += 1;
                        }
                        continue;
                    }
                }
            }
        }

        // Check for property / field: ends with `;`
        if let Some(before_semi_raw) = declaration_line.strip_suffix(';') {
            let before_semi = before_semi_raw.trim();
            let before_assign = before_semi.split('=').next().unwrap_or(before_semi).trim();
            let before_colon = before_assign
                .split(':')
                .next()
                .unwrap_or(before_assign)
                .trim();
            let ident = before_colon.split_whitespace().last().unwrap_or("");
            if !ident.is_empty()
                && ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                && ident != "return"
                && ident != "break"
            {
                let member_end = member_start + declaration_start + declaration_line.len();
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
                // A decorator may start several lines before the field declaration.
                // Consume its complete range so the field is never recorded twice.
                while i < lines.len() && line_offsets[i] < member_end {
                    i += 1;
                }
                continue;
            }
        }

        i += 1;
    }

    members
}

/// Balance parameter parentheses using the TypeScript lexer, including backtick
/// text and executable template interpolations, rather than counting literal bytes.
fn typescript_parameters_end(text: &str, open: usize) -> Option<usize> {
    if text.as_bytes().get(open) != Some(&b'(') {
        return None;
    }
    let code = crate::caller_migration::lexical_code_mask(
        text,
        crate::parameter_object::Language::TypeScript,
    );
    let mut depth = 0usize;
    for (offset, byte) in text.bytes().enumerate().skip(open) {
        if !code[offset] {
            continue;
        }
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(offset);
                }
            }
            _ => {}
        }
    }
    None
}
