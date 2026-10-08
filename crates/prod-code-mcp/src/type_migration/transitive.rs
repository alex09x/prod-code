/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::call_matching::{collect_candidate_files, find_matching_call_params, sync_cpp_headers};
use super::matching::find_matching_vars;
use super::sites::display;
use crate::parameter_object::Language;
use crate::type_migration::matching::returns::find_matching_caller_vars;
use crate::type_migration::matching::returns::find_matching_return;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

/// Transitively propagate type changes along data-flow edges (variables, return signatures, parameters, fields).
#[allow(clippy::too_many_arguments)]
pub(crate) fn propagate_transitive(
    root: &Path,
    rewritten: &mut BTreeMap<PathBuf, String>,
    initial_file: &Path,
    initial_name: &str,
    was: &str,
    to: &str,
    lang: Language,
    also: &mut Vec<PathBuf>,
) -> (usize, Vec<String>) {
    let mut queue: VecDeque<(PathBuf, String, String, String)> = VecDeque::new();
    let mut visited: BTreeSet<(PathBuf, String)> = BTreeSet::new();
    let mut migrated_descriptions = Vec::new();

    queue.push_back((
        initial_file.to_path_buf(),
        initial_name.to_string(),
        was.to_string(),
        to.to_string(),
    ));
    visited.insert((initial_file.to_path_buf(), initial_name.to_string()));

    while let Some((cur_file, cur_name, cur_was, cur_to)) = queue.pop_front() {
        let cur_flang = Language::of(&cur_file).unwrap_or(lang);
        let text = match rewritten.get(&cur_file) {
            Some(t) => t.clone(),
            None => match std::fs::read_to_string(&cur_file) {
                Ok(t) => {
                    rewritten.insert(cur_file.clone(), t.clone());
                    t
                }
                Err(_) => continue,
            },
        };

        // 1. Downstream variable bindings in cur_file
        let var_matches = find_matching_vars(&text, &cur_name, &cur_was, cur_flang);
        if !var_matches.is_empty() {
            let mut updated_text = text.clone();
            let mut sorted_vars = var_matches;
            sorted_vars.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
            for (s, e, var_name) in sorted_vars {
                updated_text.replace_range(s..e, &cur_to);
                migrated_descriptions.push(format!(
                    "{}:{var_name} (var {cur_was} → {cur_to})",
                    display(root, &cur_file)
                ));
                if !visited.contains(&(cur_file.clone(), var_name.clone())) {
                    visited.insert((cur_file.clone(), var_name.clone()));
                    queue.push_back((cur_file.clone(), var_name, cur_was.clone(), cur_to.clone()));
                }
            }
            rewritten.insert(cur_file.clone(), updated_text);
        }

        // 2. Return statements in cur_file
        let text_after_vars = rewritten.get(&cur_file).cloned().unwrap_or(text);
        if let Some((s, e, fn_name)) =
            find_matching_return(&text_after_vars, &cur_name, &cur_was, cur_flang)
        {
            let mut updated_text = text_after_vars.clone();
            updated_text.replace_range(s..e, &cur_to);
            rewritten.insert(cur_file.clone(), updated_text);
            migrated_descriptions.push(format!(
                "{}:{fn_name} (return {cur_was} → {cur_to})",
                display(root, &cur_file)
            ));

            if matches!(cur_flang, Language::Cpp | Language::C) {
                sync_cpp_headers(
                    root, &cur_file, &fn_name, &cur_was, &cur_to, rewritten, also, true,
                );
            }

            if !visited.contains(&(cur_file.clone(), fn_name.clone())) {
                visited.insert((cur_file.clone(), fn_name.clone()));
                queue.push_back((cur_file.clone(), fn_name, cur_was.clone(), cur_to.clone()));
            }
        }

        // 3. Call sites across workspace passing cur_name as argument to another function
        let candidate_files = collect_candidate_files(root, rewritten, &cur_name);
        for f in candidate_files {
            let f_text = match rewritten.get(&f) {
                Some(t) => t.clone(),
                None => match std::fs::read_to_string(&f) {
                    Ok(t) => t,
                    Err(_) => continue,
                },
            };
            let f_lang = Language::of(&f).unwrap_or(lang);

            let call_param_matches =
                find_matching_call_params(root, &f_text, &cur_name, &cur_was, f_lang, rewritten);
            for (callee_file, p_start, p_end, param_name, callee_name) in call_param_matches {
                let mut c_text = match rewritten.get(&callee_file) {
                    Some(t) => t.clone(),
                    None => match std::fs::read_to_string(&callee_file) {
                        Ok(t) => t,
                        Err(_) => continue,
                    },
                };
                c_text.replace_range(p_start..p_end, &cur_to);
                rewritten.insert(callee_file.clone(), c_text);
                if !also.contains(&callee_file) {
                    also.push(callee_file.clone());
                }
                migrated_descriptions.push(format!(
                    "{}:{callee_name}({param_name}) (param {cur_was} → {cur_to})",
                    display(root, &callee_file)
                ));

                let c_lang = Language::of(&callee_file).unwrap_or(lang);
                if matches!(c_lang, Language::Cpp | Language::C) {
                    sync_cpp_headers(
                        root,
                        &callee_file,
                        &callee_name,
                        &cur_was,
                        &cur_to,
                        rewritten,
                        also,
                        false,
                    );
                }

                if !visited.contains(&(callee_file.clone(), param_name.clone())) {
                    visited.insert((callee_file.clone(), param_name.clone()));
                    queue.push_back((
                        callee_file.clone(),
                        param_name,
                        cur_was.clone(),
                        cur_to.clone(),
                    ));
                }
            }

            // Callers assigning return value of cur_name
            let caller_var_matches =
                find_matching_caller_vars(&f_text, &cur_name, &cur_was, f_lang);
            if !caller_var_matches.is_empty() {
                let mut f_updated = f_text.clone();
                let mut sorted_cv = caller_var_matches;
                sorted_cv.sort_by_key(|(s, _, _)| std::cmp::Reverse(*s));
                for (s, e, var_name) in sorted_cv {
                    f_updated.replace_range(s..e, &cur_to);
                    migrated_descriptions.push(format!(
                        "{}:{var_name} (caller var {cur_was} → {cur_to})",
                        display(root, &f)
                    ));
                    if !visited.contains(&(f.clone(), var_name.clone())) {
                        visited.insert((f.clone(), var_name.clone()));
                        queue.push_back((f.clone(), var_name, cur_was.clone(), cur_to.clone()));
                    }
                }
                rewritten.insert(f.clone(), f_updated);
                if !also.contains(&f) {
                    also.push(f.clone());
                }
            }
        }
    }

    let count = migrated_descriptions.len();
    (count, migrated_descriptions)
}
