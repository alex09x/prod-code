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

fn find_default_label(s: &str) -> Option<(usize, usize)> {
    let mut offset = 0;
    while let Some(idx) = s[offset..].find("default") {
        let abs = offset + idx;
        let after = &s[abs + 7..];
        let trimmed_len = after.len() - after.trim_start().len();
        if after[trimmed_len..].starts_with(':') {
            return Some((abs, abs + 7 + trimmed_len + 1));
        }
        offset = abs + 7;
    }
    None
}

fn is_switch_keyword(text: &str, at: usize) -> bool {
    let after = &text[at + "switch".len()..];
    (at == 0
        || !text[..at]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_'))
        && !after
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric() || c == '_')
        && !crate::inline_parameter::is_in_comment(
            text,
            at,
            crate::parameter_object::Language::TypeScript,
        )
        && !crate::inline_parameter::is_in_string(
            text,
            at,
            crate::parameter_object::Language::TypeScript,
        )
}

/// Parse a `switch` statement in C-like languages (TypeScript, JavaScript, C++, Swift, Go).
pub fn parse_switch_block(text: &str, search_offset: usize) -> Option<ConditionalBlock> {
    let switch_keyword_idx = text
        .match_indices("switch")
        .map(|(at, _)| at)
        .find(|at| *at >= search_offset && is_switch_keyword(text, *at))
        .or_else(|| {
            text.match_indices("switch")
                .map(|(at, _)| at)
                .filter(|at| *at < search_offset && is_switch_keyword(text, *at))
                .last()
        })?;

    let open_brace = text[switch_keyword_idx..].find('{')? + switch_keyword_idx;
    let close_brace = crate::pull_push::find_matching_brace(text, open_brace)?;

    let header = text[switch_keyword_idx..open_brace].trim();
    let discriminator = if let Some(open_paren) = header.find('(')
        && let Some(close_paren) = header.rfind(')')
    {
        header[open_paren + 1..close_paren].trim().to_string()
    } else {
        header.strip_prefix("switch")?.trim().to_string()
    };

    let inner = &text[open_brace + 1..close_brace];
    let indent = line_indentation(text, switch_keyword_idx);

    // Split inner body into cases
    let mut branches = Vec::new();
    let mut pos = 0;

    while pos < inner.len() {
        let rest = &inner[pos..];
        let next_case = rest.find("case ");
        let next_default = find_default_label(rest);

        let (is_default, tag, body_start) = match (next_case, next_default) {
            (Some(c), Some((d, _))) if c < d => {
                let tag_start = pos + c + 5;
                let colon_rel = inner[tag_start..].find(':')?;
                let colon_idx = tag_start + colon_rel;
                let tag = inner[tag_start..colon_idx].trim().to_string();
                (false, tag, colon_idx + 1)
            }
            (Some(_), Some((_, d_end))) => (true, "default".to_string(), pos + d_end),
            (Some(c), None) => {
                let tag_start = pos + c + 5;
                let colon_rel = inner[tag_start..].find(':')?;
                let colon_idx = tag_start + colon_rel;
                let tag = inner[tag_start..colon_idx].trim().to_string();
                (false, tag, colon_idx + 1)
            }
            (None, Some((_, d_end))) => (true, "default".to_string(), pos + d_end),
            (None, None) => break,
        };

        // Body extends to next case/default or end of block
        let rem = &inner[body_start..];
        let next_c = rem.find("case ");
        let next_d = find_default_label(rem).map(|(s, _)| s);
        let next_marker = match (next_c, next_d) {
            (Some(c), Some(d)) => Some(c.min(d)),
            (Some(c), None) => Some(c),
            (None, Some(d)) => Some(d),
            (None, None) => None,
        };
        let body_end = if let Some(m) = next_marker {
            body_start + m
        } else {
            inner.len()
        };

        let raw_body = inner[body_start..body_end].trim();
        // Clean break statements from body
        let clean_body = raw_body
            .lines()
            .filter(|l| {
                let t = l.trim();
                t != "break;" && t != "break"
            })
            .collect::<Vec<_>>()
            .join("\n");

        branches.push(ConditionalBranch {
            variant_name: tag_to_variant_name(&tag),
            tag,
            body: clean_body,
            is_default,
        });

        if body_end <= pos {
            break;
        }
        pos = body_end;
    }

    if branches.is_empty() {
        return None;
    }

    Some(ConditionalBlock {
        kind: ConditionalKind::Switch,
        start_offset: switch_keyword_idx,
        end_offset: close_brace + 1,
        discriminator,
        branches,
        indent,
    })
}
