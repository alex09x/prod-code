/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::format::{adjust_indentation, cpp_member_access, replace_python_pass};
use super::languages::parse_classes_in_text;
use super::types::{ClassDecl, HierarchyRefactorResult};
use super::workspace::find_subclasses_in_workspace;
use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub async fn push_down_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    class_name: &str,
    target_classes_opt: Option<&[String]>,
    members_to_push: &[String],
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<HierarchyRefactorResult> {
    if members_to_push.is_empty() {
        bail!("No members specified to push down");
    }

    let super_file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let classes_in_super_file = parse_classes_in_text(&super_file_text, &language, file);
    let super_class = classes_in_super_file
        .iter()
        .find(|c| c.name == class_name)
        .cloned()
        .with_context(|| format!("class '{class_name}' not found in {}", file.display()))?;

    // Verify members exist in superclass
    let mut members_to_move = Vec::new();
    for name in members_to_push {
        let member = super_class
            .members
            .iter()
            .find(|m| m.name == *name)
            .cloned()
            .with_context(|| format!("member '{name}' not found in superclass '{class_name}'"))?;
        members_to_move.push(member);
    }

    // Discover target subclasses
    let candidate_subclasses = find_subclasses_in_workspace(root, class_name, &language);
    let target_subclasses: Vec<(PathBuf, String, ClassDecl)> = match target_classes_opt {
        Some(targets) if !targets.is_empty() => {
            let mut matched = Vec::new();
            for t in targets {
                if let Some(c) = candidate_subclasses
                    .iter()
                    .find(|(_, _, cls)| cls.name == *t)
                {
                    matched.push(c.clone());
                } else if !force {
                    bail!(
                        "specified target subclass '{t}' not found as a subclass of '{class_name}'"
                    );
                }
            }
            matched
        }
        _ => {
            if candidate_subclasses.is_empty() && !force {
                bail!("no subclasses of '{class_name}' found in workspace to push down to");
            }
            candidate_subclasses
        }
    };

    let target_class_names: Vec<String> = target_subclasses
        .iter()
        .map(|(_, _, c)| c.name.clone())
        .collect();

    // Check collisions in target subclasses
    for (_, _, sub) in &target_subclasses {
        for m in members_to_push {
            if let Some(existing) = sub.members.iter().find(|mem| mem.name == *m)
                && !force
            {
                bail!(
                    "target subclass '{}' already defines member '{}'",
                    sub.name,
                    existing.name
                );
            }
        }
    }

    // Track overlays
    let mut file_contents: BTreeMap<PathBuf, String> = BTreeMap::new();

    // 1. Remove members from superclass
    let mut modified_super = super_file_text.clone();
    let mut sorted_members = members_to_move.clone();
    sorted_members.sort_by_key(|m| std::cmp::Reverse(m.start_offset));

    for m in sorted_members {
        let mut start = m.start_offset;
        let mut end = m.end_offset;
        if end < modified_super.len() && modified_super.as_bytes()[end] == b'\n' {
            end += 1;
        } else if start > 0 && modified_super.as_bytes()[start - 1] == b'\n' {
            start -= 1;
        }
        modified_super.replace_range(start..end, "");
    }

    if language == "python"
        && let Some(updated_super) = parse_classes_in_text(&modified_super, &language, file)
            .into_iter()
            .find(|c| c.name == class_name)
    {
        let body_slice = &modified_super[updated_super.body_start..updated_super.body_end];
        if body_slice.trim().is_empty() {
            let pass_stmt = format!("{}pass\n", updated_super.indent);
            modified_super.insert_str(updated_super.body_start, &pass_stmt);
        }
    }
    file_contents.insert(file.to_path_buf(), modified_super);

    // 2. Insert members into each target subclass
    for (sub_path, sub_content, sub_class) in target_subclasses {
        let current_text = file_contents.get(&sub_path).cloned().unwrap_or(sub_content);

        // Adjust member indentation for this subclass
        let mut prepared = Vec::new();
        for m in &members_to_move {
            let source = if language == "cpp" {
                format!(
                    "{}:\n{}",
                    cpp_member_access(&super_file_text, &super_class, m),
                    m.full_text
                )
            } else {
                m.full_text.clone()
            };
            let adjusted = adjust_indentation(&source, &sub_class.indent);
            prepared.push(adjusted);
        }
        let insert_block = prepared.join("\n\n");

        let mut modified_sub = current_text;
        // Re-parse subclass in current text to get latest offsets
        let current_classes = parse_classes_in_text(&modified_sub, &language, &sub_path);
        if let Some(cur_sub) = current_classes
            .into_iter()
            .find(|c| c.name == sub_class.name)
        {
            if language == "python" {
                let body_slice = &modified_sub[cur_sub.body_start..cur_sub.body_end];
                if body_slice.trim() == "pass" {
                    replace_python_pass(&mut modified_sub, &cur_sub, &insert_block);
                } else {
                    let insert_pos = cur_sub.body_end;
                    let formatted = format!("\n\n{insert_block}");
                    modified_sub.insert_str(insert_pos, &formatted);
                }
            } else {
                let insert_pos = cur_sub.body_end;
                let formatted = format!("\n{insert_block}\n");
                modified_sub.insert_str(insert_pos, &formatted);
            }
            file_contents.insert(sub_path, modified_sub);
        }
    }

    // Build unified diff and overlays
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
        operation: "push_down".to_string(),
        source_class: class_name.to_string(),
        target_classes: target_class_names,
        members: members_to_push.to_vec(),
        files_modified,
        overlays,
        diff: diff_output,
        applied: apply,
        verified,
        diagnostics,
    })
}
