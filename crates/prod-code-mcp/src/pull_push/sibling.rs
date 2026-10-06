/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::format::normalized_member_text;
use super::languages::parse_classes_in_text;
use super::types::MemberDecl;
use super::workspace::find_subclasses_in_workspace;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub(crate) fn clean_siblings_impl(
    root: &Path,
    super_name: &str,
    class_name: &str,
    language: &str,
    member_decls_to_move: &[MemberDecl],
    file_contents: &mut BTreeMap<PathBuf, String>,
) {
    let candidate_paths: BTreeSet<PathBuf> =
        find_subclasses_in_workspace(root, super_name, language)
            .into_iter()
            .map(|(p, _, _)| p)
            .collect();

    for sib_path in candidate_paths {
        let mut modified_text = file_contents
            .get(&sib_path)
            .cloned()
            .unwrap_or_else(|| std::fs::read_to_string(&sib_path).unwrap_or_default());

        let mut any_changed = false;
        loop {
            let current_classes = parse_classes_in_text(&modified_text, language, &sib_path);
            let sibling_with_target_member = current_classes.into_iter().find(|c| {
                c.name != class_name
                    && c.super_names.iter().any(|s| s == super_name)
                    && c.members.iter().any(|sibling_member| {
                        member_decls_to_move.iter().any(|pulled_member| {
                            sibling_member.name == pulled_member.name
                                && normalized_member_text(sibling_member, language)
                                    == normalized_member_text(pulled_member, language)
                        })
                    })
            });

            let Some(sib_class) = sibling_with_target_member else {
                break;
            };

            let matching_members: Vec<MemberDecl> = sib_class
                .members
                .into_iter()
                .filter(|sibling_member| {
                    member_decls_to_move.iter().any(|pulled_member| {
                        sibling_member.name == pulled_member.name
                            && normalized_member_text(sibling_member, language)
                                == normalized_member_text(pulled_member, language)
                    })
                })
                .collect();

            let mut sorted = matching_members;
            sorted.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

            for m in sorted {
                let mut start = m.start_offset;
                let mut end = m.end_offset;
                if end < modified_text.len() && modified_text.as_bytes()[end] == b'\n' {
                    end += 1;
                } else if start > 0 && modified_text.as_bytes()[start - 1] == b'\n' {
                    start -= 1;
                }
                modified_text.replace_range(start..end, "");
            }

            if language == "python"
                && let Some(updated_sib) =
                    parse_classes_in_text(&modified_text, language, &sib_path)
                        .into_iter()
                        .find(|c| c.name == sib_class.name)
            {
                let body_slice = &modified_text[updated_sib.body_start..updated_sib.body_end];
                if body_slice.trim().is_empty() {
                    let pass_stmt = format!("{}pass\n", updated_sib.indent);
                    modified_text.insert_str(updated_sib.body_start, &pass_stmt);
                }
            }
            any_changed = true;
        }

        if any_changed {
            file_contents.insert(sib_path, modified_text);
        }
    }
}
