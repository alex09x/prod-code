/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::format::{
    adjust_indentation, cpp_member_access, replace_python_pass, strip_override_modifiers,
};
use super::languages::parse_classes_in_text;
use super::sibling::clean_siblings_impl;
use super::types::HierarchyRefactorResult;
use super::workspace::find_class_in_workspace;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub async fn pull_up_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    class_name: &str,
    target_class_opt: Option<&str>,
    members_to_pull: &[String],
    clean_siblings: bool,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<HierarchyRefactorResult> {
    if members_to_pull.is_empty() {
        bail!("No members specified to pull up");
    }

    let sub_file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let classes_in_sub_file = parse_classes_in_text(&sub_file_text, &language, file);
    let sub_class = classes_in_sub_file
        .iter()
        .find(|c| c.name == class_name)
        .cloned()
        .with_context(|| format!("class '{class_name}' not found in {}", file.display()))?;

    // Determine target superclass
    let super_name = match target_class_opt {
        Some(t) => {
            if !sub_class.super_names.iter().any(|s| s == t) && !force {
                bail!(
                    "class '{class_name}' does not list '{t}' as a superclass (known superclasses: {:?})",
                    sub_class.super_names
                );
            }
            t.to_string()
        }
        None => {
            if sub_class.super_names.is_empty() {
                bail!("class '{class_name}' does not declare any superclass");
            }
            if sub_class.super_names.len() > 1 {
                bail!(
                    "class '{class_name}' has multiple superclasses {:?}; please specify 'target_class'",
                    sub_class.super_names
                );
            }
            sub_class.super_names[0].clone()
        }
    };

    // Locate superclass declaration
    let (super_file, super_file_text, super_class) = if let Some(c) =
        classes_in_sub_file.iter().find(|c| c.name == super_name)
    {
        (file.to_path_buf(), sub_file_text.clone(), c.clone())
    } else {
        find_class_in_workspace(root, &super_name, &language).with_context(|| {
            format!("superclass '{super_name}' not found in workspace for language '{language}'")
        })?
    };

    // Validate members exist in subclass and do NOT collide in superclass
    let mut member_decls_to_move = Vec::new();
    for name in members_to_pull {
        let member = sub_class
            .members
            .iter()
            .find(|m| m.name == *name)
            .cloned()
            .with_context(|| format!("member '{name}' not found in class '{class_name}'"))?;

        if let Some(existing) = super_class.members.iter().find(|m| m.name == *name)
            && !force
        {
            bail!(
                "superclass '{super_name}' already defines member '{name}' (start at offset {})",
                existing.start_offset
            );
        }
        member_decls_to_move.push(member);
    }

    // Prepare moved members text with stripped override modifiers and adjusted indentation
    let mut prepared_members = Vec::new();
    for m in &member_decls_to_move {
        let stripped = strip_override_modifiers(&m.full_text, &language);
        let source = if language == "cpp" {
            format!(
                "{}:\n{stripped}",
                cpp_member_access(&sub_file_text, &sub_class, m)
            )
        } else {
            stripped
        };
        let adjusted = adjust_indentation(&source, &super_class.indent);
        prepared_members.push(adjusted);
    }

    let insert_block = prepared_members.join("\n\n");

    // Overlays to produce: file -> new content
    let mut file_contents: BTreeMap<PathBuf, String> = BTreeMap::new();
    let same_file = file == super_file;

    if same_file {
        let mut modified = sub_file_text.clone();

        // 1. Remove members from subclass (in reverse offset order to keep indices valid)
        let mut sorted_members = member_decls_to_move.clone();
        sorted_members.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

        for m in sorted_members {
            // Include leading or trailing newline in removal
            let mut start = m.start_offset;
            let mut end = m.end_offset;
            if end < modified.len() && modified.as_bytes()[end] == b'\n' {
                end += 1;
            } else if start > 0 && modified.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            modified.replace_range(start..end, "");
        }

        // Check if subclass body became completely empty in Python
        if language == "python" {
            // Re-parse to see if subclass body is empty
            if let Some(updated_sub) = parse_classes_in_text(&modified, &language, file)
                .into_iter()
                .find(|c| c.name == class_name)
            {
                let body_slice = &modified[updated_sub.body_start..updated_sub.body_end];
                if body_slice.trim().is_empty() {
                    let pass_stmt = format!("{}pass\n", updated_sub.indent);
                    modified.insert_str(updated_sub.body_start, &pass_stmt);
                }
            }
        }

        // 2. Insert into superclass body
        // Re-parse classes in modified text to get updated superclass offsets
        let updated_classes = parse_classes_in_text(&modified, &language, file);
        let updated_super = updated_classes
            .into_iter()
            .find(|c| c.name == super_name)
            .with_context(|| format!("cannot re-locate superclass '{super_name}' after edits"))?;

        if language == "python" {
            let body_slice = &modified[updated_super.body_start..updated_super.body_end];
            if body_slice.trim() == "pass" {
                replace_python_pass(&mut modified, &updated_super, &insert_block);
            } else {
                let insert_pos = updated_super.body_end;
                let formatted = format!("\n\n{insert_block}");
                modified.insert_str(insert_pos, &formatted);
            }
        } else {
            // TS / C++ / Swift / Rust: insert before closing brace
            let insert_pos = updated_super.body_end;
            let formatted = format!("\n{insert_block}\n");
            modified.insert_str(insert_pos, &formatted);
        }

        file_contents.insert(file.to_path_buf(), modified);
    } else {
        // Multi-file: sub_file and super_file are distinct
        // 1. Edit sub_file
        let mut modified_sub = sub_file_text.clone();
        let mut sorted_members = member_decls_to_move.clone();
        sorted_members.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

        for m in sorted_members {
            let mut start = m.start_offset;
            let mut end = m.end_offset;
            if end < modified_sub.len() && modified_sub.as_bytes()[end] == b'\n' {
                end += 1;
            } else if start > 0 && modified_sub.as_bytes()[start - 1] == b'\n' {
                start -= 1;
            }
            modified_sub.replace_range(start..end, "");
        }

        if language == "python"
            && let Some(updated_sub) = parse_classes_in_text(&modified_sub, &language, file)
                .into_iter()
                .find(|c| c.name == class_name)
        {
            let body_slice = &modified_sub[updated_sub.body_start..updated_sub.body_end];
            if body_slice.trim().is_empty() {
                let pass_stmt = format!("{}pass\n", updated_sub.indent);
                modified_sub.insert_str(updated_sub.body_start, &pass_stmt);
            }
        }
        file_contents.insert(file.to_path_buf(), modified_sub);

        // 2. Edit super_file
        let mut modified_super = super_file_text.clone();
        if language == "python" {
            let body_slice = &modified_super[super_class.body_start..super_class.body_end];
            if body_slice.trim() == "pass" {
                replace_python_pass(&mut modified_super, &super_class, &insert_block);
            } else {
                let insert_pos = super_class.body_end;
                let formatted = format!("\n\n{insert_block}");
                modified_super.insert_str(insert_pos, &formatted);
            }
        } else {
            let insert_pos = super_class.body_end;
            let formatted = format!("\n{insert_block}\n");
            modified_super.insert_str(insert_pos, &formatted);
        }
        file_contents.insert(super_file.clone(), modified_super);
    }

    // 3. Sibling cleanup if requested
    if clean_siblings {
        clean_siblings_impl(
            root,
            &super_name,
            class_name,
            &language,
            &member_decls_to_move,
            &mut file_contents,
        );
    }

    // Prepare unified diff and overlays
    let mut diff_output = String::new();
    let mut overlays = Vec::new();
    let mut files_modified = Vec::new();

    for (p, new_text) in &file_contents {
        let old_text = std::fs::read_to_string(p).unwrap_or_default();
        let rel_path = p
            .strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .to_string();
        files_modified.push(rel_path.clone());
        overlays.push((p.clone(), new_text.clone()));

        let text_diff = similar::TextDiff::from_lines(&old_text, new_text);
        let patch = text_diff
            .unified_diff()
            .header(&format!("a/{rel_path}"), &format!("b/{rel_path}"))
            .to_string();
        diff_output.push_str(&patch);
    }

    // In-memory analyzer validation
    let reports = crate::diagnostics::validate_texts(remote, root, &overlays, &[]).await?;
    let mut diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    // Optional compiler verification
    let mut verified = false;
    if verify == Some("compile") {
        let files_to_compile: Vec<(String, String)> = overlays
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect();
        let check = crate::compile_check::check(remote, root, &files_to_compile).await?;
        verified = check.passed;
        if !check.passed {
            if !force {
                bail!("compiler verification failed:\n{}", check.errors.join("\n"));
            }
            diagnostics.push(format!("compiler errors: {}", check.errors.join("; ")));
        }
    }

    // Apply if requested
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the refactoring produces analyzer errors; nothing was written:\n  {}",
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&file_contents);
        crate::refactor::apply_workspace_edit(root, &edit)?;
    }

    Ok(HierarchyRefactorResult {
        operation: "pull_up".to_string(),
        source_class: class_name.to_string(),
        target_classes: vec![super_name],
        members: members_to_pull.to_vec(),
        files_modified,
        overlays,
        diff: diff_output,
        applied: apply,
        verified,
        diagnostics,
    })
}
