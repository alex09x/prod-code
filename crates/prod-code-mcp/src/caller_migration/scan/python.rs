/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::safety::{find_python_body_close, verify_all_usages_safe};
use super::super::types::{CallerMigration, ReplacementCandidate, is_ident};
use crate::parameter_object::Language;

pub(crate) fn scan_python(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
    candidates: &mut Vec<ReplacementCandidate>,
) {
    // Find `ident:\s*type_name\b` or `ident:\s*"type_name"`
    for (pos, _) in text.match_indices(type_name) {
        // Word boundaries
        let before_c = text[..pos].chars().next_back().unwrap_or(' ');
        let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
        if is_ident(before_c) || is_ident(after_c) {
            continue;
        }
        if matches!(after_c, '[' | '|' | ']') {
            continue;
        }

        let is_quoted = before_c == '"' || before_c == '\'';
        let check_start = if is_quoted { pos - 1 } else { pos };
        let before = text[..check_start].trim_end();
        if !before.ends_with(':') {
            continue;
        }
        let before_colon = before[..before.len() - 1].trim_end();
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
                "self" | "cls" | "def" | "class" | "return" | "yield"
            )
        {
            continue;
        }

        let prefix_var = before_colon[..before_colon.len() - var_name.len()].trim_end();
        if prefix_var.ends_with(')') || prefix_var.ends_with("->") {
            continue;
        }

        // Scope: find the def header's colon and the indented body
        let _def_pos = match text[..pos].rfind("def ") {
            Some(d) => d,
            None => continue,
        };
        let colon_pos = match text[pos..].find(':') {
            Some(c) => pos + c,
            None => continue,
        };
        let scope_end = find_python_body_close(text, colon_pos);
        let scope_start = colon_pos + 1;

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
