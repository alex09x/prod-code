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

pub(crate) fn scan_rust(
    text: &str,
    type_name: &str,
    interface_name: &str,
    extracted_methods: &[String],
    lang: Language,
    mask: &[bool],
    candidates: &mut Vec<ReplacementCandidate>,
) {
    if type_name.contains('<') || interface_name.contains('<') {
        return;
    }
    // Find `ident:\s*&mut\s+type_name\b`, `ident:\s*&type_name\b`, `ident:\s*type_name\b`
    for (pos, _) in text.match_indices(type_name) {
        if !mask[pos] {
            continue;
        }
        let before_c = text[..pos].chars().next_back().unwrap_or(' ');
        let after_c = text[pos + type_name.len()..].chars().next().unwrap_or(' ');
        if is_ident(before_c) || is_ident(after_c) {
            continue;
        }
        if matches!(after_c, '<' | '[' | ':') {
            continue;
        }

        let before_type = text[..pos].trim_end();
        let (has_ref, has_mut, type_start) = if let Some(r) = before_type.strip_suffix("&mut") {
            (true, true, pos - (before_type.len() - r.len()))
        } else if let Some(r) = before_type.strip_suffix('&') {
            (true, false, pos - (before_type.len() - r.len()))
        } else {
            (false, false, pos)
        };

        let before_ref = text[..type_start].trim_end();
        if !before_ref.ends_with(':') {
            continue;
        }
        let before_colon = before_ref[..before_ref.len() - 1].trim_end();
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
                "self" | "Self" | "fn" | "let" | "mut" | "pub" | "return"
            )
        {
            continue;
        }

        // Scope: find `{` after parameter list
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
            let replacement = if has_mut {
                format!("&mut impl {interface_name}")
            } else if has_ref {
                format!("&impl {interface_name}")
            } else {
                format!("impl {interface_name}")
            };

            candidates.push(ReplacementCandidate {
                start: type_start,
                end: pos + type_name.len(),
                replacement: replacement.clone(),
                migration: CallerMigration {
                    var_name,
                    original_type: text[type_start..pos + type_name.len()].trim().to_string(),
                    new_type: replacement,
                },
            });
        }
    }
}
