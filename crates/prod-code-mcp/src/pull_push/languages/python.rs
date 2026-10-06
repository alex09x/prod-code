/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::pull_push::types::{ClassDecl, MemberDecl, MemberKind};
use std::path::Path;

pub fn parse_python_classes(text: &str, file_path: &Path) -> Vec<ClassDecl> {
    let mut classes = Vec::new();
    let lines: Vec<&str> = text.split('\n').collect();
    let mut line_starts = Vec::with_capacity(lines.len());
    let mut offset = 0;
    for l in &lines {
        line_starts.push(offset);
        offset += l.len() + 1; // +1 for \n
    }

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("class ") && trimmed_start.contains(':') {
            let base_indent_len = line.len() - trimmed_start.len();
            let base_indent = &line[..base_indent_len];
            let after_class = trimmed_start["class ".len()..].trim_start();
            let colon_idx = match after_class.find(':') {
                Some(idx) => idx,
                None => {
                    i += 1;
                    continue;
                }
            };
            let class_header = after_class[..colon_idx].trim();
            let (name, super_names) = if let Some(paren_idx) = class_header.find('(') {
                let name = class_header[..paren_idx].trim().to_string();
                let bases_str = if class_header.ends_with(')') {
                    &class_header[paren_idx + 1..class_header.len() - 1]
                } else {
                    &class_header[paren_idx + 1..]
                };
                let bases: Vec<String> = bases_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                (name, bases)
            } else {
                (class_header.to_string(), Vec::new())
            };

            let decl_start = line_starts[i];
            let body_start_line = i + 1;

            // Find body extent and member indentation
            let mut body_end_line = body_start_line;
            let mut member_indent = None;

            let mut j = body_start_line;
            while j < lines.len() {
                let body_line = lines[j];
                let trimmed = body_line.trim();
                if trimmed.is_empty() {
                    j += 1;
                    continue;
                }
                let current_indent_len = body_line.len() - body_line.trim_start().len();
                if current_indent_len <= base_indent_len {
                    break;
                }
                if member_indent.is_none() && !trimmed.starts_with('#') {
                    member_indent = Some(body_line[..current_indent_len].to_string());
                }
                body_end_line = j + 1;
                j += 1;
            }

            let body_indent = member_indent.unwrap_or_else(|| format!("{base_indent}    "));
            let decl_end = if body_end_line < lines.len() {
                line_starts[body_end_line]
            } else {
                text.len()
            };
            let body_start = if body_start_line < lines.len() {
                line_starts[body_start_line]
            } else {
                text.len()
            };
            let body_end = decl_end;

            // Parse members inside Python class
            let mut members = Vec::new();
            let mut k = body_start_line;
            while k < body_end_line {
                let cur = lines[k];
                let trimmed = cur.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    k += 1;
                    continue;
                }

                if cur.starts_with(&body_indent) {
                    let after_indent = &cur[body_indent.len()..];
                    // Check for decorators
                    let member_start_line = if after_indent.starts_with('@') {
                        let start_dec = k;
                        while k < body_end_line && lines[k].trim_start().starts_with('@') {
                            k += 1;
                        }
                        start_dec
                    } else {
                        k
                    };

                    if k >= body_end_line {
                        break;
                    }

                    let def_line = lines[k];
                    let def_trimmed = def_line.trim();
                    if let Some(after_def_raw) = def_trimmed.strip_prefix("def ") {
                        let after_def = after_def_raw.trim_start();
                        if let Some(paren_idx) = after_def.find('(') {
                            let member_name = after_def[..paren_idx].trim().to_string();
                            let member_start = line_starts[member_start_line];

                            // Method body extends until next line at body_indent
                            let mut method_end_line = k + 1;
                            while method_end_line < body_end_line {
                                let m_line = lines[method_end_line];
                                let m_trimmed = m_line.trim();
                                if m_trimmed.is_empty() || m_trimmed.starts_with('#') {
                                    method_end_line += 1;
                                    continue;
                                }
                                let m_indent_len = m_line.len() - m_line.trim_start().len();
                                if m_indent_len <= body_indent.len() {
                                    break;
                                }
                                method_end_line += 1;
                            }

                            // Trim trailing blank lines from member end
                            let mut last_non_blank = method_end_line;
                            while last_non_blank > member_start_line
                                && lines[last_non_blank - 1].trim().is_empty()
                            {
                                last_non_blank -= 1;
                            }

                            let member_end = if last_non_blank < lines.len() {
                                line_starts[last_non_blank - 1] + lines[last_non_blank - 1].len()
                            } else {
                                text.len()
                            };

                            let full_text = text[member_start..member_end].to_string();
                            members.push(MemberDecl {
                                name: member_name,
                                kind: MemberKind::Method,
                                is_override: false,
                                start_offset: member_start,
                                end_offset: member_end,
                                full_text,
                            });
                            k = method_end_line;
                            continue;
                        }
                    } else if after_indent.contains('=') || after_indent.contains(':') {
                        // Field / Constant
                        let first_ident = after_indent
                            .split(&[':', '=', ' '][..])
                            .next()
                            .unwrap_or("")
                            .trim();
                        if !first_ident.is_empty()
                            && first_ident.chars().all(|c| c.is_alphanumeric() || c == '_')
                        {
                            let member_start = line_starts[k];
                            let member_end = line_starts[k] + lines[k].len();
                            let full_text = text[member_start..member_end].to_string();
                            members.push(MemberDecl {
                                name: first_ident.to_string(),
                                kind: MemberKind::Field,
                                is_override: false,
                                start_offset: member_start,
                                end_offset: member_end,
                                full_text,
                            });
                        }
                    }
                }
                k += 1;
            }

            classes.push(ClassDecl {
                name,
                language: "python".to_string(),
                file_path: file_path.to_path_buf(),
                super_names,
                decl_start,
                decl_end,
                body_start,
                body_end,
                indent: body_indent,
                members,
            });

            i = body_end_line;
            continue;
        }
        i += 1;
    }

    classes
}
