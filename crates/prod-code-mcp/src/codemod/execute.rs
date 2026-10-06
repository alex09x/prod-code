/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use super::matcher::{find_structural_matches_in_source, rewrite_source};
use super::scope::{collect_code_files, validate_scope_path};
use super::types::{CodemodOutcome, CodemodRule, CompiledPattern, StructuralSearchResult};

/// Run structural codemod across a file or workspace checkout.
pub fn run_codemod(
    workspace_root: &Path,
    rule_str: &str,
    scope: Option<&Path>,
    apply: bool,
) -> Result<CodemodOutcome> {
    let start = Instant::now();
    let rule = CodemodRule::parse(rule_str)?;
    let workspace_root = workspace_root
        .canonicalize()
        .context("cannot resolve workspace root")?;

    let mut target_files = Vec::new();
    if let Some(target) = scope {
        let target = validate_scope_path(&workspace_root, target)?;
        if target.is_file() {
            target_files.push(target.to_path_buf());
        } else if target.is_dir() {
            collect_code_files(&target, &mut target_files);
        }
    } else {
        collect_code_files(&workspace_root, &mut target_files);
    }

    target_files.sort();

    let files_scanned = target_files.len();
    let mut rewritten_files = Vec::new();
    let mut unified_diffs = String::new();
    let mut changed_lines = 0;

    for path in &target_files {
        let Ok(old_text) = std::fs::read_to_string(path) else {
            continue;
        };

        if let Some(new_text) = rewrite_source(&old_text, &rule) {
            let rel = path
                .strip_prefix(&workspace_root)
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_else(|_| path.to_string_lossy().into_owned());

            let diff = similar::TextDiff::from_lines(&old_text, &new_text);
            let file_changed = diff
                .iter_all_changes()
                .filter(|c| c.tag() != similar::ChangeTag::Equal)
                .count();
            changed_lines += file_changed;

            unified_diffs.push_str(
                &diff
                    .unified_diff()
                    .context_radius(2)
                    .header(&format!("a/{rel}"), &format!("b/{rel}"))
                    .to_string(),
            );

            rewritten_files.push((path.clone(), new_text));
        }
    }

    let files_matched = rewritten_files.len();

    // If apply is requested and there are rewritten files, apply them atomically
    if apply && !rewritten_files.is_empty() {
        let mut document_changes = Vec::new();
        for (p, new_text) in &rewritten_files {
            let uri = url::Url::from_file_path(p)
                .map_err(|_| anyhow::anyhow!("invalid path {:?}", p))?
                .to_string();
            document_changes.push(serde_json::json!({
                "textDocument": { "uri": uri },
                "edits": [{
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 999999, "character": 0 },
                    },
                    "newText": new_text,
                }],
            }));
        }
        let workspace_edit = serde_json::json!({ "documentChanges": document_changes });
        crate::refactor::apply_workspace_edit(&workspace_root, &workspace_edit)?;
    }

    let elapsed = start.elapsed();

    Ok(CodemodOutcome {
        rule: rule_str.to_string(),
        files_scanned,
        files_matched,
        total_matches: files_matched,
        changed_lines,
        diff: unified_diffs,
        rewritten_files,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    })
}

/// Run read-only structural AST search across files in the workspace.
pub fn run_structural_search(
    workspace_root: &Path,
    pattern_str: &str,
    scope: Option<&Path>,
) -> Result<StructuralSearchResult> {
    let start = Instant::now();
    let pattern = CompiledPattern::parse(pattern_str)?;

    let mut target_files = Vec::new();
    if let Some(target) = scope {
        if target.is_file() {
            target_files.push(target.to_path_buf());
        } else if target.is_dir() {
            collect_code_files(target, &mut target_files);
        } else {
            let abs = workspace_root.join(target);
            if abs.is_file() {
                target_files.push(abs);
            } else if abs.is_dir() {
                collect_code_files(&abs, &mut target_files);
            }
        }
    } else {
        collect_code_files(workspace_root, &mut target_files);
    }

    target_files.sort();
    let files_scanned = target_files.len();
    let mut all_matches = Vec::new();
    let mut matched_files_set = HashSet::new();

    for path in &target_files {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };

        let rel = path
            .strip_prefix(workspace_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_string_lossy().into_owned());

        let file_matches = find_structural_matches_in_source(&rel, &source, &pattern);
        if !file_matches.is_empty() {
            matched_files_set.insert(rel);
            all_matches.extend(file_matches);
        }
    }

    let elapsed = start.elapsed();

    Ok(StructuralSearchResult {
        pattern: pattern_str.to_string(),
        files_scanned,
        files_matched: matched_files_set.len(),
        total_matches: all_matches.len(),
        matches: all_matches,
        elapsed_ms: elapsed.as_secs_f64() * 1000.0,
    })
}
