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

/// Parse a Rust `match` statement.
pub fn parse_rust_match(text: &str, search_offset: usize) -> Option<ConditionalBlock> {
    let match_kw_idx = text[search_offset..]
        .find("match ")
        .map(|idx| search_offset + idx)
        .or_else(|| text[..search_offset].rfind("match "))?;

    let open_brace = text[match_kw_idx..].find('{')? + match_kw_idx;
    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)?;

    let header = text[match_kw_idx..open_brace].trim();
    let discriminator = header.strip_prefix("match")?.trim().to_string();

    let inner = &text[open_brace + 1..close_brace];
    let indent = line_indentation(text, match_kw_idx);

    let mut branches = Vec::new();
    let mut pos = 0;

    while pos < inner.len() {
        let rest = &inner[pos..];
        let Some(arrow_idx) = rest.find("=>") else {
            break;
        };
        let pattern_part = rest[..arrow_idx].trim();
        let pattern = pattern_part.lines().last().unwrap_or(pattern_part).trim();
        if pattern.is_empty() {
            break;
        }

        let is_default = pattern == "_";
        let after_arrow = &rest[arrow_idx + 2..];
        let after_arrow_trimmed = after_arrow.trim_start();
        let leading_spaces = after_arrow.len() - after_arrow_trimmed.len();
        let body_start_rel = arrow_idx + 2 + leading_spaces;

        let (body, branch_end_rel) = if after_arrow_trimmed.starts_with('{') {
            let brace_in_inner = pos + body_start_rel;
            let matching = crate::pull_push::find_matching_brace(inner, brace_in_inner)?;
            let b = inner[brace_in_inner + 1..matching].trim().to_string();
            let mut end_rel = matching + 1 - pos;
            if inner[matching + 1..].starts_with(',') {
                end_rel += 1;
            }
            (b, end_rel)
        } else {
            let next_comma = find_rust_arm_comma(after_arrow);
            let end_rel = if let Some(c) = next_comma {
                arrow_idx + 2 + c + 1
            } else {
                rest.len()
            };
            let b = rest[arrow_idx + 2..if let Some(c) = next_comma {
                arrow_idx + 2 + c
            } else {
                rest.len()
            }]
                .trim()
                .to_string();
            (b, end_rel)
        };

        branches.push(ConditionalBranch {
            variant_name: tag_to_variant_name(pattern),
            tag: pattern.to_string(),
            body,
            is_default,
        });

        if branch_end_rel == 0 {
            break;
        }
        pos += branch_end_rel;
    }

    if branches.is_empty() {
        return None;
    }

    Some(ConditionalBlock {
        kind: ConditionalKind::Switch,
        start_offset: match_kw_idx,
        end_offset: close_brace + 1,
        discriminator,
        branches,
        indent,
    })
}

fn find_rust_arm_comma(expression: &str) -> Option<usize> {
    let mut paren = 0usize;
    let mut bracket = 0usize;
    let mut brace = 0usize;
    let mut angle = 0usize;
    let mut previous = String::new();
    for (i, ch) in expression.char_indices() {
        if crate::extract_field::is_in_literal_or_comment(
            expression,
            i,
            crate::parameter_object::Language::Rust,
        ) {
            continue;
        }
        match ch {
            '(' => paren += 1,
            ')' => paren = paren.saturating_sub(1),
            '[' => bracket += 1,
            ']' => bracket = bracket.saturating_sub(1),
            '{' => brace += 1,
            '}' => brace = brace.saturating_sub(1),
            '<' if angle > 0 || previous.trim_end().ends_with("::") => angle += 1,
            '>' if angle > 0 => angle -= 1,
            ',' if paren == 0 && bracket == 0 && brace == 0 && angle == 0 => return Some(i),
            _ => {}
        }
        if ch.is_whitespace() {
            previous.push(ch);
        } else {
            previous.push(ch);
            if previous.len() > 4 {
                let drain_to = previous.len() - 4;
                previous.drain(..drain_to);
            }
        }
    }
    None
}
