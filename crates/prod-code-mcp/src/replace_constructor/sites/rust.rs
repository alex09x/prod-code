/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::parse::{is_ident, is_ident_str, split_balanced_commas};
use super::super::types::InstantiationSite;
use std::collections::BTreeMap;

/// Finds raw struct instantiations of `type_name` in `text` for Rust.
pub fn find_rust_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> (Vec<InstantiationSite>, Vec<String>) {
    let mut sites = Vec::new();
    let mut blocked = Vec::new();
    let target_self_ranges = crate::extract_field::impl_blocks(text)
        .into_iter()
        .filter_map(|(impl_type, _, open, close)| {
            let base = impl_type.split('<').next().unwrap_or(&impl_type).trim();
            let base = base.rsplit("::").next().unwrap_or(base).trim();
            (base == type_name).then_some((open, close))
        })
        .collect::<Vec<_>>();

    let matches: Vec<(usize, &str)> = text
        .match_indices(type_name)
        .chain(text.match_indices("Self"))
        .collect();

    for (at, name) in matches {
        if crate::extract_field::is_in_literal_or_comment(
            text,
            at,
            crate::parameter_object::Language::Rust,
        ) {
            continue;
        }
        if at >= decl_start && at < decl_end {
            continue;
        }
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + name.len()..].chars().next().is_some_and(is_ident) {
            continue;
        }
        if name == "Self"
            && !target_self_ranges
                .iter()
                .any(|(open, close)| *open < at && at < *close)
        {
            continue;
        }
        let line_start = text[..at].rfind('\n').map_or(0, |i| i + 1);
        let before = text[line_start..at].trim();
        if before.starts_with("struct")
            || before.starts_with("enum")
            || before.starts_with("impl")
            || before.starts_with("type ")
            || before.ends_with("->")
            || before.ends_with("for")
        {
            continue;
        }

        // Check if there is an opening brace following `name`
        let Some(brace) = crate::extract_field::constructor_brace(text, at, name) else {
            continue;
        };
        let Some(close) = crate::parameter_object::matching_bracket(text, brace) else {
            continue;
        };

        // Check if it is a pattern rather than an expression
        if matches!(
            crate::extract_field::braces_kind(text, brace, close),
            crate::extract_field::Braces::Pattern { .. }
        ) {
            continue;
        }

        let inner = &text[brace + 1..close];
        let has_rest = inner.contains("..");
        if has_rest {
            let (l, c) = crate::signature::position_at(text, at).unwrap_or((0, 0));
            blocked.push(format!(
                "{l}:{c} instantiation uses struct update syntax `..` which cannot be mapped to positional constructor arguments"
            ));
            continue;
        }

        let mut field_values = BTreeMap::new();
        let mut field_order = Vec::new();
        let chunks = split_balanced_commas(inner);
        for chunk in chunks {
            let trimmed = chunk.trim();
            if trimmed.is_empty() || trimmed.starts_with("//") {
                continue;
            }
            if let Some((f, val)) = trimmed.split_once(':') {
                field_values.insert(f.trim().to_string(), val.trim().to_string());
                field_order.push(f.trim().to_string());
            } else if is_ident_str(trimmed) {
                // Shorthand field: `name` is `name: name`
                field_values.insert(trimmed.to_string(), trimmed.to_string());
                field_order.push(trimmed.to_string());
            }
        }

        // Determine path prefix before `name`, e.g. `crate::models::`
        let mut prefix_start = at;
        let bytes = text.as_bytes();
        while prefix_start >= 2 && &text[prefix_start - 2..prefix_start] == "::" {
            let mut j = prefix_start - 2;
            while j > 0 && is_ident(bytes[j - 1] as char) {
                j -= 1;
            }
            prefix_start = j;
        }
        let prefix = text[prefix_start..at].to_string();

        sites.push(InstantiationSite {
            start: prefix_start,
            end: close + 1,
            field_values,
            field_order,
            prefix,
            has_rest_pattern: false,
        });
    }

    sites.sort_by_key(|s| s.start);
    sites.dedup_by_key(|s| s.start);
    (sites, blocked)
}
