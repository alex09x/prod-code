/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::types::{
    ConditionalBlock, ConditionalBranch, ConditionalKind, line_indentation, tag_to_variant_name,
};

/// Parse an `if / else if / else` or Python `if / elif / else` cascade.
pub fn parse_if_else_block(text: &str, search_offset: usize) -> Option<ConditionalBlock> {
    let if_idx = text[search_offset..]
        .find("if ")
        .map(|idx| search_offset + idx)
        .or_else(|| text[..search_offset].rfind("if "))?;

    let if_line_start = text[..if_idx].rfind('\n').map_or(0, |i| i + 1);
    let indent = line_indentation(text, if_idx);
    let is_python = text[if_idx..].find(':').is_some()
        && (text[if_idx..].find('{').is_none()
            || text[if_idx..].find(':').unwrap() < text[if_idx..].find('{').unwrap());

    if is_python {
        parse_python_if_elif(text, if_line_start, &indent)
    } else {
        parse_curly_if_else(text, if_idx, &indent)
    }
}

fn parse_python_if_elif(text: &str, if_idx: usize, base_indent: &str) -> Option<ConditionalBlock> {
    let mut branches = Vec::new();
    let mut discriminator = String::new();

    let lines: Vec<&str> = text[if_idx..].lines().collect();
    let mut end_line_count = 0;
    let mut current_tag = String::new();
    let mut current_body_lines = Vec::new();
    let mut is_default = false;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let line_indent = line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let base_indent_len = base_indent.len();

        if line_indent == base_indent_len
            && (trimmed.starts_with("if ")
                || trimmed.starts_with("elif ")
                || trimmed.starts_with("else:"))
        {
            // Flush previous branch
            if !current_tag.is_empty() {
                branches.push(ConditionalBranch {
                    variant_name: tag_to_variant_name(&current_tag),
                    tag: current_tag.clone(),
                    body: current_body_lines.join("\n"),
                    is_default,
                });
                current_body_lines.clear();
            }

            if trimmed.starts_with("if ") || trimmed.starts_with("elif ") {
                let cond_part = if let Some(rest) = trimmed.strip_prefix("if ") {
                    rest.strip_suffix(':').unwrap_or(rest).trim()
                } else if let Some(rest) = trimmed.strip_prefix("elif ") {
                    rest.strip_suffix(':').unwrap_or(rest).trim()
                } else {
                    ""
                };

                // Parse `x == "VAL"` or `x == VAL`
                if let Some((lhs, rhs)) = cond_part.split_once("==") {
                    if discriminator.is_empty() {
                        discriminator = lhs.trim().to_string();
                    }
                    current_tag = rhs.trim().to_string();
                    is_default = false;
                } else {
                    current_tag = cond_part.to_string();
                    is_default = false;
                }
            } else if trimmed.starts_with("else:") {
                current_tag = "default".to_string();
                is_default = true;
            }
            end_line_count = idx + 1;
        } else if line_indent > base_indent_len {
            current_body_lines.push(*line);
            end_line_count = idx + 1;
        } else if idx > 0 && !trimmed.is_empty() {
            // Finished the cascade
            break;
        }
    }

    if !current_tag.is_empty() {
        branches.push(ConditionalBranch {
            variant_name: tag_to_variant_name(&current_tag),
            tag: current_tag,
            body: current_body_lines.join("\n"),
            is_default,
        });
    }

    if branches.len() < 2 {
        return None;
    }

    // Compute end offset from end_line_count
    let mut end_offset = if_idx;
    for l in lines.iter().take(end_line_count) {
        end_offset += l.len() + 1;
    }
    end_offset = end_offset.min(text.len());

    Some(ConditionalBlock {
        kind: ConditionalKind::IfElse,
        start_offset: if_idx,
        end_offset,
        discriminator,
        branches,
        indent: base_indent.to_string(),
    })
}

fn parse_curly_if_else(text: &str, if_idx: usize, base_indent: &str) -> Option<ConditionalBlock> {
    let mut branches = Vec::new();
    let mut discriminator = String::new();
    let mut pos = if_idx;
    let mut last_end = if_idx;

    while pos < text.len() {
        let rest = text[pos..].trim_start();
        if rest.starts_with("if ")
            || rest.starts_with("else if ")
            || rest.starts_with("if(")
            || rest.starts_with("else if(")
        {
            let is_else_if = rest.starts_with("else if");
            let after_kw = if is_else_if { &rest[7..] } else { &rest[2..] };
            let open_brace = after_kw.find('{')?;
            let header = after_kw[..open_brace].trim();

            let cond_str = if let Some(op) = header.find('(')
                && let Some(cp) = header.rfind(')')
            {
                &header[op + 1..cp]
            } else {
                header
            };

            let tag = if let Some((lhs, rhs)) = cond_str
                .split_once("===")
                .or_else(|| cond_str.split_once("=="))
            {
                if discriminator.is_empty() {
                    discriminator = lhs.trim().to_string();
                }
                rhs.trim().to_string()
            } else {
                cond_str.trim().to_string()
            };

            let brace_global = text[pos..].find('{')? + pos;
            let close_brace = crate::pull_push::find_matching_brace(text, brace_global)?;
            let body = text[brace_global + 1..close_brace].trim().to_string();

            branches.push(ConditionalBranch {
                variant_name: tag_to_variant_name(&tag),
                tag,
                body,
                is_default: false,
            });

            last_end = close_brace + 1;
            pos = last_end;
        } else if rest.starts_with("else") && (rest[4..].trim_start().starts_with('{')) {
            let brace_global = text[pos..].find('{')? + pos;
            let close_brace = crate::pull_push::find_matching_brace(text, brace_global)?;
            let body = text[brace_global + 1..close_brace].trim().to_string();

            branches.push(ConditionalBranch {
                variant_name: "Default".to_string(),
                tag: "default".to_string(),
                body,
                is_default: true,
            });

            last_end = close_brace + 1;
            break;
        } else {
            break;
        }
    }

    if branches.len() < 2 {
        return None;
    }

    Some(ConditionalBlock {
        kind: ConditionalKind::IfElse,
        start_offset: if_idx,
        end_offset: last_end,
        discriminator,
        branches,
        indent: base_indent.to_string(),
    })
}
