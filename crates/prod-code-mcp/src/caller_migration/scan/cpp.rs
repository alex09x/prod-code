/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::safety::verify_all_usages_safe;
use super::super::types::{CallerMigration, ReplacementCandidate, is_ident};
use crate::parameter_object::Language;
use crate::pull_push::find_matching_brace;

pub(crate) fn scan_cpp(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
    candidates: &mut Vec<ReplacementCandidate>,
) {
    // Find `const type_name& ident`, `type_name* ident`, `const type_name* ident`, etc.
    for (pos, _) in text.match_indices(type_name) {
        if !mask[pos] {
            continue;
        }
        let before_c = text[..pos].chars().next_back().unwrap_or(' ');
        let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
        if is_ident(before_c) || is_ident(after_c) {
            continue;
        }

        // Check preceding `const`
        let before_type = text[..pos].trim_end();
        let (has_const, start_idx) = if before_type.ends_with("const") {
            let const_start = before_type.len() - 5;
            let before_const = &before_type[..const_start];
            if before_const.is_empty() || !is_ident(before_const.chars().next_back().unwrap()) {
                (true, const_start)
            } else {
                (false, pos)
            }
        } else {
            (false, pos)
        };

        // Check following `&` or `*`
        let after_type = &text[pos + type_name.len()..];
        let trimmed_after = after_type.trim_start();
        let (has_ref, has_ptr, rest) = if let Some(r) = trimmed_after.strip_prefix('&') {
            (true, false, r.trim_start())
        } else if let Some(p) = trimmed_after.strip_prefix('*') {
            (false, true, p.trim_start())
        } else {
            (false, false, trimmed_after)
        };

        let var_name: String = rest.chars().take_while(|c| is_ident(*c)).collect();
        if var_name.is_empty()
            || matches!(
                var_name.as_str(),
                "class" | "struct" | "void" | "int" | "double" | "return"
            )
        {
            continue;
        }
        if !has_ref && !has_ptr {
            continue; // Changing a by-value parameter into a reference changes copy/move behavior.
        }

        // End of type annotation span
        let end_idx = pos
            + type_name.len()
            + (after_type.len() - trimmed_after.len())
            + usize::from(has_ref || has_ptr);

        // Scope: find `{` after `)`
        let open_paren = match text[..pos].rfind('(') {
            Some(p) => p,
            None => continue,
        };
        let close_paren = match text[pos..].find(')') {
            Some(p) => pos + p,
            None => continue,
        };
        if pos < open_paren || pos > close_paren {
            continue;
        }
        let open_brace = match text[close_paren..].find('{') {
            Some(b) => close_paren + b,
            None => continue,
        };
        let close_brace = match find_matching_brace(text, open_brace) {
            Some(b) => b,
            None => continue,
        };

        if verify_all_usages_safe(
            text,
            open_brace + 1,
            close_brace,
            &var_name,
            extracted_methods,
            lang,
            mask,
        ) {
            let replacement = if has_ptr {
                if has_const {
                    format!("const {interface_name}*")
                } else {
                    format!("{interface_name}*")
                }
            } else if has_const || !has_ref {
                format!("const {interface_name}&")
            } else {
                format!("{interface_name}&")
            };

            candidates.push(ReplacementCandidate {
                start: start_idx,
                end: end_idx,
                replacement: replacement.clone(),
                migration: CallerMigration {
                    var_name,
                    original_type: text[start_idx..end_idx].trim().to_string(),
                    new_type: replacement,
                },
            });
        }
    }
}
