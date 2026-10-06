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

pub(crate) fn scan_go(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
    candidates: &mut Vec<ReplacementCandidate>,
) {
    // Find `ident *type_name` or `ident type_name` inside `func`
    for (pos, _) in text.match_indices(type_name) {
        if !mask[pos] {
            continue;
        }
        let before_c = text[..pos].chars().next_back().unwrap_or(' ');
        let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
        if is_ident(before_c) || is_ident(after_c) {
            continue;
        }
        if after_c == '[' {
            continue;
        }

        let has_pointer = before_c == '*';
        let start_idx = if has_pointer { pos - 1 } else { pos };
        let before_type = text[..start_idx].trim_end();

        let var_name: String = before_type
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
                "func" | "type" | "var" | "const" | "return" | "struct" | "interface"
            )
        {
            continue;
        }

        // Scope: enclosing func
        let func_pos = match text[..start_idx].rfind("func ") {
            Some(f) => f,
            None => continue,
        };
        // Make sure pos is in parameter list of that func
        let open_paren = match text[func_pos..].find('(') {
            Some(p) => func_pos + p,
            None => continue,
        };
        let close_paren = match text[open_paren..].find(')') {
            Some(p) => open_paren + p,
            None => continue,
        };

        let is_receiver = text[func_pos + 5..open_paren].trim().is_empty();
        let (param_open, param_close) = if is_receiver {
            if start_idx <= close_paren {
                // Method receiver in Go cannot be an interface type
                continue;
            }
            let next_open = match text[close_paren..].find('(') {
                Some(p) => close_paren + p,
                None => continue,
            };
            let next_close = match text[next_open..].find(')') {
                Some(p) => next_open + p,
                None => continue,
            };
            (next_open, next_close)
        } else {
            (open_paren, close_paren)
        };

        if start_idx < param_open || start_idx > param_close {
            continue;
        }

        let open_brace = match text[param_close..].find('{') {
            Some(b) => param_close + b,
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
                start: start_idx,
                end: pos + type_name.len(),
                replacement: interface_name.to_string(),
                migration: CallerMigration {
                    var_name,
                    original_type: if has_pointer {
                        format!("*{type_name}")
                    } else {
                        type_name.to_string()
                    },
                    new_type: interface_name.to_string(),
                },
            });
        }
    }
}
