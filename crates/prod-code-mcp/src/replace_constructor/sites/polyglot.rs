/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::parse::{is_ident, split_balanced_commas};
use super::super::types::InstantiationSite;
use std::collections::BTreeMap;

/// Finds raw instantiations in Go (`&Type{...}` or `Type{...}`).
pub fn find_go_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    for (at, _) in text.match_indices(type_name) {
        if crate::extract_field::is_in_literal_or_comment(
            text,
            at,
            crate::parameter_object::Language::Go,
        ) {
            continue;
        }
        if at >= decl_start && at < decl_end {
            continue;
        }
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..]
            .chars()
            .next()
            .is_some_and(is_ident)
        {
            continue;
        }
        let after = &text[at + type_name.len()..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('{') {
            continue;
        }
        let open = at + type_name.len() + after.len() - trimmed_after.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };

        let start = if at > 0 && text.as_bytes()[at - 1] == b'&' {
            at - 1
        } else {
            at
        };

        let mut field_values = BTreeMap::new();
        let mut field_order = Vec::new();
        let inner = &text[open + 1..close];
        let mut positional_index = 0usize;
        for chunk in split_balanced_commas(inner) {
            let trimmed = chunk.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Some((k, v)) = trimmed.split_once(':') {
                field_values.insert(k.trim().to_string(), v.trim().to_string());
                field_order.push(k.trim().to_string());
            } else {
                let key = format!("__positional_{positional_index}");
                field_values.insert(key.clone(), trimmed.to_string());
                field_order.push(key);
                positional_index += 1;
            }
        }

        sites.push(InstantiationSite {
            start,
            end: close + 1,
            field_values,
            field_order,
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Finds raw instantiations in TypeScript / JavaScript (`new Type(...)`).
pub fn find_ts_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    let pat = format!("new {type_name}");
    for (at, _) in text.match_indices(&pat) {
        if crate::extract_field::is_in_literal_or_comment(
            text,
            at,
            crate::parameter_object::Language::TypeScript,
        ) {
            continue;
        }
        if at >= decl_start && at < decl_end {
            continue;
        }
        let after = &text[at + pat.len()..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('(') {
            continue;
        }
        let open = at + pat.len() + after.len() - trimmed_after.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let mut field_values = BTreeMap::new();
        field_values.insert(
            "__raw_args__".to_string(),
            text[open + 1..close].trim().to_string(),
        );
        sites.push(InstantiationSite {
            start: at,
            end: close + 1,
            field_values,
            field_order: Vec::new(),
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Finds direct C++ object initializations (`T{...}` and `T(...)`).
pub fn find_cpp_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    for (at, _) in text.match_indices(type_name) {
        if crate::extract_field::is_in_literal_or_comment(
            text,
            at,
            crate::parameter_object::Language::Cpp,
        ) || (at >= decl_start && at < decl_end)
            || (at > 0 && text[..at].chars().next_back().is_some_and(is_ident))
            || text[at + type_name.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
        {
            continue;
        }
        let after = &text[at + type_name.len()..];
        let trimmed = after.trim_start();
        let Some(_) = trimmed.chars().next().filter(|c| matches!(c, '{' | '(')) else {
            continue;
        };
        let open = at + type_name.len() + after.len() - trimmed.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let mut field_values = BTreeMap::new();
        field_values.insert(
            "__raw_args__".to_string(),
            text[open + 1..close].trim().to_string(),
        );
        sites.push(InstantiationSite {
            start: at,
            end: close + 1,
            field_values,
            field_order: Vec::new(),
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Finds Swift memberwise initializer calls (`T(...)`).
pub fn find_swift_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    for (at, _) in text.match_indices(type_name) {
        if crate::extract_field::is_in_literal_or_comment(
            text,
            at,
            crate::parameter_object::Language::Swift,
        ) || (at >= decl_start && at < decl_end)
            || (at > 0 && text[..at].chars().next_back().is_some_and(is_ident))
            || text[at + type_name.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
        {
            continue;
        }
        let after = &text[at + type_name.len()..];
        let trimmed = after.trim_start();
        if !trimmed.starts_with('(') {
            continue;
        }
        let open = at + type_name.len() + after.len() - trimmed.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let mut field_values = BTreeMap::new();
        field_values.insert(
            "__raw_args__".to_string(),
            text[open + 1..close].trim().to_string(),
        );
        sites.push(InstantiationSite {
            start: at,
            end: close + 1,
            field_values,
            field_order: Vec::new(),
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}

/// Finds raw instantiations in Python (`Type(...)`).
pub fn find_python_instantiations(
    text: &str,
    type_name: &str,
    decl_start: usize,
    decl_end: usize,
) -> Vec<InstantiationSite> {
    let mut sites = Vec::new();
    for (at, _) in text.match_indices(type_name) {
        if crate::extract_field::is_in_literal_or_comment(
            text,
            at,
            crate::parameter_object::Language::Python,
        ) {
            continue;
        }
        if at >= decl_start && at < decl_end {
            continue;
        }
        if at > 0 && text[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        if text[at + type_name.len()..]
            .chars()
            .next()
            .is_some_and(is_ident)
        {
            continue;
        }
        let before = text[..at].trim_end();
        if before.ends_with("class") || before.ends_with("def") || before.ends_with("import") {
            continue;
        }
        let after = &text[at + type_name.len()..];
        let trimmed_after = after.trim_start();
        if !trimmed_after.starts_with('(') {
            continue;
        }
        let open = at + type_name.len() + after.len() - trimmed_after.len();
        let Some(close) = crate::parameter_object::matching_bracket(text, open) else {
            continue;
        };
        let mut field_values = BTreeMap::new();
        field_values.insert(
            "__raw_args__".to_string(),
            text[open + 1..close].trim().to_string(),
        );
        sites.push(InstantiationSite {
            start: at,
            end: close + 1,
            field_values,
            field_order: Vec::new(),
            prefix: String::new(),
            has_rest_pattern: false,
        });
    }
    sites
}
