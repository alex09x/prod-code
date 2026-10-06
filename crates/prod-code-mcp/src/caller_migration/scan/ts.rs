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

pub(crate) fn scan_ts(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
    candidates: &mut Vec<ReplacementCandidate>,
) {
    // Find `ident\s*(\?)?\s*:\s*type_name\b`
    for (pos, _) in text.match_indices(type_name) {
        if !mask[pos] {
            continue;
        }
        // Check word boundaries
        let before_c = text[..pos].chars().next_back().unwrap_or(' ');
        let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
        if is_ident(before_c) || is_ident(after_c) {
            continue;
        }
        // Do not match arrays, generics, or unions
        if matches!(after_c, '[' | '<' | '>' | '|' | '&') {
            continue;
        }

        // Check preceding `:`
        let before = text[..pos].trim_end();
        if !before.ends_with(':') {
            continue;
        }
        let before_colon = before[..before.len() - 1].trim_end();
        let before_colon = before_colon.trim_end_matches('?').trim_end();

        // Extract var_name
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
                "return"
                    | "function"
                    | "class"
                    | "interface"
                    | "type"
                    | "case"
                    | "default"
                    | "export"
                    | "import"
            )
        {
            continue;
        }

        // Ensure it's not a return type (preceded by `)`)
        let prefix_var = before_colon[..before_colon.len() - var_name.len()].trim_end();
        if prefix_var.ends_with(')') || prefix_var.ends_with("=>") {
            continue;
        }

        // Determine scope
        let scope_opt = if let Some(open_paren) = text[..pos].rfind('(') {
            if let Some(close_paren) = text[open_paren..].find(')') {
                let abs_close = open_paren + close_paren;
                if pos < abs_close {
                    // Inside parameter list: find `{` after `)`
                    if let Some(brace_offset) = text[abs_close..].find('{') {
                        let open_brace = abs_close + brace_offset;
                        find_matching_brace(text, open_brace)
                            .map(|close_brace| (open_brace + 1, close_brace))
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let (scope_start, scope_end) = match scope_opt {
            Some(s) => s,
            None => {
                // Check if it's a variable declaration in an enclosing block
                if let Some(open_brace) = text[..pos].rfind('{') {
                    if let Some(close_brace) = find_matching_brace(text, open_brace) {
                        (pos + type_name.len(), close_brace)
                    } else {
                        continue;
                    }
                } else {
                    continue;
                }
            }
        };

        if verify_all_usages_safe(
            text,
            scope_start,
            scope_end,
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
