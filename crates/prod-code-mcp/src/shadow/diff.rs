/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::HypothesisSpec;
use anyhow::{Context, Result};
use std::path::Path;

/// Unified diff of the hypothesis against the checkout and the number of changed lines.
pub fn unified_diff(root: &Path, spec: &HypothesisSpec) -> (String, usize) {
    let mut out = String::new();
    let mut changed = 0;
    for edit in &spec.edits {
        let path = root.join(&edit.relative_path);
        let exists = path.exists();
        let old = std::fs::read_to_string(&path).unwrap_or_default();
        let new = edit.text.clone().unwrap_or_default();
        if old == new && exists == edit.text.is_some() {
            continue;
        }
        let diff = similar::TextDiff::from_lines(&old, &new);
        changed += diff
            .iter_all_changes()
            .filter(|c| c.tag() != similar::ChangeTag::Equal)
            .count();
        let a = if exists {
            format!("a/{}", edit.relative_path)
        } else {
            "/dev/null".to_string()
        };
        let b = if edit.text.is_some() {
            format!("b/{}", edit.relative_path)
        } else {
            "/dev/null".to_string()
        };
        out.push_str(
            &diff
                .unified_diff()
                .context_radius(3)
                .header(&a, &b)
                .to_string(),
        );
    }
    (out, changed)
}

/// Writes a hypothesis into the checkout; returns the paths written or deleted.
pub fn apply_hypothesis(root: &Path, spec: &HypothesisSpec) -> Result<Vec<String>> {
    let mut touched = Vec::new();
    for edit in &spec.edits {
        let path = root.join(&edit.relative_path);
        match &edit.text {
            Some(text) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, text)
                    .with_context(|| format!("cannot write {}", path.display()))?;
            }
            None => {
                if path.exists() {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("cannot delete {}", path.display()))?;
                }
            }
        }
        touched.push(edit.relative_path.clone());
    }
    Ok(touched)
}
