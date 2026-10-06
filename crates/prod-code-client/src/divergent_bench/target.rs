/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{DivergentTarget, Language};
use anyhow::{Result, anyhow, bail};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| anyhow!("failed to spawn `git {}` in {:?}: {e}", args.join(" "), dir))?;
    if !output.status.success() {
        bail!(
            "`git {}` in {:?} failed: {}",
            args.join(" "),
            dir,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub(crate) fn is_candidate_source(rel: &str, language: Language) -> bool {
    let ext_ok = Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e == language.extension());
    if !ext_ok {
        return false;
    }
    let lower = rel.to_ascii_lowercase();
    let excluded_dirs = [
        "test",
        "bench",
        "example",
        "vendor",
        "target",
        "node_modules",
        "third_party",
        "proto",
        "generated",
        ".bak",
    ];
    if lower
        .split('/')
        .any(|seg| excluded_dirs.iter().any(|ex| seg.contains(ex)))
    {
        return false;
    }
    match language {
        Language::Rust => !lower.ends_with("build.rs"),
        Language::Go => !lower.ends_with("_test.go") && !lower.ends_with(".pb.go"),
    }
}

/// Finds a single-line function signature on `line` and returns `(symbol, name_col)`.
pub(crate) fn parse_signature_line(line: &str, language: Language) -> Option<(String, usize)> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    let after_keyword = match language {
        Language::Rust => trimmed.strip_prefix("pub fn ")?,
        Language::Go => {
            let rest = trimmed.strip_prefix("func ")?;
            if rest.starts_with('(') {
                return None; // method receiver; keep to plain functions
            }
            rest
        }
    };
    let name_end = after_keyword.find(['(', '<'])?;
    let name = &after_keyword[..name_end];
    // Entry points hover as package/crate documentation rather than as a signature.
    if matches!(name, "main" | "init") {
        return None;
    }
    if name.is_empty()
        || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        || name.starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    if !trimmed.contains('(') || !trimmed.trim_end().ends_with('{') {
        return None;
    }
    let open = trimmed.find('(')?;
    let close = matching_paren(trimmed, open)?;
    if trimmed[open..=close].contains("...") {
        return None;
    }
    let name_col = indent + (trimmed.len() - after_keyword.len());
    let _ = close;
    Some((name.to_string(), name_col))
}

pub(crate) fn matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text.char_indices().skip(open) {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

/// Discovers the first single-line top-level function in a tracked, non-test source file.
pub fn discover_target(root: &Path, language: Language) -> Result<DivergentTarget> {
    let listing = run_git(root, &["ls-files", "-z"])?;
    let mut candidates: Vec<&str> = listing
        .split('\0')
        .filter(|rel| !rel.is_empty() && is_candidate_source(rel, language))
        .collect();
    // Prefer conventional source roots so the symbol sits inside the indexed crate/package tree.
    candidates.sort_by_key(|rel| {
        let depth = rel.matches('/').count();
        let in_src = rel.starts_with("src/") || rel.contains("/src/");
        (if in_src { 0 } else { 1 }, depth, rel.to_string())
    });

    for rel in candidates {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if language == Language::Rust && line.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            if let Some((symbol, _)) = parse_signature_line(line, language) {
                return Ok(DivergentTarget {
                    language,
                    file_rel: PathBuf::from(rel),
                    symbol,
                    line: idx,
                });
            }
        }
    }
    bail!(
        "no single-line `{}` signature found in tracked {} sources under {root:?}",
        match language {
            Language::Rust => "pub fn",
            Language::Go => "func",
        },
        language.label()
    )
}

/// Rewrites `line` so the parameter list ends with the language's marker parameter.
pub fn mutate_signature(line: &str, language: Language) -> Result<String> {
    let open = line
        .find('(')
        .ok_or_else(|| anyhow!("signature has no parameter list: {line}"))?;
    let close =
        matching_paren(line, open).ok_or_else(|| anyhow!("unbalanced parameter list: {line}"))?;
    let params = line[open + 1..close].trim();
    let marker = language.marker_param();
    let new_params = if params.is_empty() {
        marker.to_string()
    } else if params.ends_with(',') {
        format!("{params} {marker}")
    } else {
        format!("{params}, {marker}")
    };
    Ok(format!(
        "{}({}){}",
        &line[..open],
        new_params,
        &line[close + 1..]
    ))
}

pub(crate) fn go_package_name(content: &str) -> Option<String> {
    content.lines().find_map(|line| {
        line.trim()
            .strip_prefix("package ")
            .map(|p| p.split_whitespace().next().unwrap_or("").to_string())
    })
}
