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

pub(crate) fn scan_swift(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
    candidates: &mut Vec<ReplacementCandidate>,
) {
    // Find `ident:\s*(any\s+)?type_name\b`
    for (pos, _) in text.match_indices(type_name) {
        if !mask[pos] {
            continue;
        }
        let before_c = text[..pos].chars().next_back().unwrap_or(' ');
        let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
        if is_ident(before_c) || is_ident(after_c) {
            continue;
        }
        if matches!(after_c, '[' | '?') {
            continue;
        }

        let before_type = text[..pos].trim_end();
        let before_type = before_type
            .strip_suffix("any")
            .unwrap_or(before_type)
            .trim_end();
        if !before_type.ends_with(':') {
            continue;
        }
        let before_colon = before_type[..before_type.len() - 1].trim_end();
        let var_name: String = before_colon
            .chars()
            .rev()
            .take_while(|c| is_ident(*c))
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if var_name.is_empty()
            || matches!(
                var_name.as_str(),
                "self" | "Self" | "func" | "class" | "struct" | "protocol" | "return"
            )
        {
            continue;
        }

        // Scope: find `{` after parameter list or declaration
        let open_brace = match text[pos..].find('{') {
            Some(b) => pos + b,
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
            candidates.push(ReplacementCandidate {
                start: pos,
                end: pos + type_name.len(),
                replacement: interface_name.to_string(),
                migration: CallerMigration {
                    var_name,
                    original_type: type_name.to_string(),
                    new_type: interface_name.to_string(),
                },
            });
        }
    }
}
