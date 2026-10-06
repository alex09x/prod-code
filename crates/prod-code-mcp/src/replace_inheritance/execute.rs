/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::pull_push::{MemberKind, find_class_in_workspace, parse_classes_in_text};

use super::languages::{transform_cpp, transform_python, transform_swift, transform_typescript};
use super::types::ReplaceInheritanceResult;

/// Core implementation for replacing inheritance with delegation.
#[allow(clippy::too_many_arguments)]
pub async fn replace_inheritance_impl(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    sub_type: &str,
    base_type_opt: Option<&str>,
    field_name_opt: Option<&str>,
    methods_opt: Option<&[String]>,
    apply: bool,
    force: bool,
    verify: Option<&str>,
) -> Result<ReplaceInheritanceResult> {
    let file_text = std::fs::read_to_string(file)
        .with_context(|| format!("cannot read file {}", file.display()))?;
    let language = crate::lang::language_id_for_path(file).to_string();

    let classes_in_file = parse_classes_in_text(&file_text, &language, file);
    let sub_class = classes_in_file
        .iter()
        .find(|c| c.name == sub_type)
        .cloned()
        .with_context(|| format!("class '{sub_type}' not found in {}", file.display()))?;

    // Determine target base class
    let base_name = match base_type_opt {
        Some(b) => {
            if !sub_class.super_names.iter().any(|s| s == b) && !force {
                bail!(
                    "class '{sub_type}' does not inherit from '{b}' (known base classes: {:?})",
                    sub_class.super_names
                );
            }
            b.to_string()
        }
        None => {
            if sub_class.super_names.is_empty() {
                bail!("class '{sub_type}' does not declare any base class");
            }
            if sub_class.super_names.len() > 1 {
                bail!(
                    "class '{sub_type}' inherits from multiple classes {:?}; please specify 'base_type'",
                    sub_class.super_names
                );
            }
            sub_class.super_names[0].clone()
        }
    };

    // Determine default field name if not specified
    let field_name = match field_name_opt {
        Some(f) => f.to_string(),
        None => match language.as_str() {
            "python" => "_base".to_string(),
            "cpp" => "base_".to_string(),
            _ => "base".to_string(),
        },
    };

    // Locate base class to discover its methods
    let base_class_opt = if let Some(b) = classes_in_file.iter().find(|c| c.name == base_name) {
        Some(b.clone())
    } else {
        find_class_in_workspace(root, &base_name, &language).map(|(_, _, c)| c)
    };

    // Determine methods to forward
    let methods_to_forward: Vec<String> = match methods_opt {
        Some(m) if !m.is_empty() => m.to_vec(),
        _ => {
            if let Some(base_cls) = &base_class_opt {
                base_cls
                    .members
                    .iter()
                    .filter(|m| {
                        m.kind == MemberKind::Method
                            && !m.name.starts_with('_')
                            && m.name != "constructor"
                            && !sub_class.members.iter().any(|sm| sm.name == m.name)
                    })
                    .map(|m| m.name.clone())
                    .collect()
            } else {
                Vec::new()
            }
        }
    };

    // Transform the subclass text
    let transformed_sub_text = match language.as_str() {
        "python" => transform_python(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        "typescript" | "javascript" => transform_typescript(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        "cpp" | "c" => transform_cpp(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        "swift" => transform_swift(
            &file_text,
            &sub_class,
            &base_name,
            &field_name,
            &methods_to_forward,
            base_class_opt.as_ref(),
        )?,
        _ => {
            bail!("replace_inheritance_with_delegation is not supported for language '{language}'")
        }
    };

    let mut file_contents: BTreeMap<PathBuf, String> = BTreeMap::new();
    file_contents.insert(file.to_path_buf(), transformed_sub_text);

    // Build diff and overlays
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

    Ok(ReplaceInheritanceResult {
        sub_type: sub_type.to_string(),
        base_type: base_name,
        field_name,
        forwarded_methods: methods_to_forward,
        files_modified,
        overlays,
        diff: diff_output,
        applied: apply,
        verified,
        diagnostics,
    })
}
