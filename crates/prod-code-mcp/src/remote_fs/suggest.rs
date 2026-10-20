/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Nearest existing workspace path suggestions when a requested file does not exist (#1125).

use std::path::{Path, PathBuf};

const SKIPPED_SUGGEST_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".build",
    "vendor",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];

const MAX_SUGGEST_SCAN_FILES: usize = 4_000;

/// Formats a file read/open context string, appending `(did you mean \`...\`?)` when a missing
/// path has a close existing relative in `root` (such as `<stem>/mod.rs` after a module split).
pub fn missing_file_context(root: &Path, path: &Path, verb: &str) -> String {
    if path.exists() {
        return format!("{verb} {}", path.display());
    }
    match suggest_nearest_path(root, path) {
        Some(hint) => format!("{verb} {} (did you mean `{hint}`?)", path.display()),
        None => format!("{verb} {}", path.display()),
    }
}

/// Suggests up to 3 nearest existing workspace-relative paths for a missing file `missing`.
pub fn suggest_nearest_path(root: &Path, missing: &Path) -> Option<String> {
    let abs_missing = if missing.is_absolute() {
        missing.to_path_buf()
    } else {
        root.join(missing)
    };
    if abs_missing.exists() {
        return None;
    }

    let rel_display = |p: &Path| -> String {
        p.strip_prefix(root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/")
    };

    // 1. Direct module-split / index-file checks in the same parent directory:
    //    `foo.rs` -> `foo/mod.rs`, `foo.ts` -> `foo/index.ts`, `foo.py` -> `foo/__init__.py`,
    //    or `foo/mod.rs` -> `foo.rs`.
    if let (Some(parent), Some(stem)) = (
        abs_missing.parent(),
        abs_missing.file_stem().and_then(|s| s.to_str()),
    ) {
        let dir_candidate = parent.join(stem);
        for index_name in ["mod.rs", "index.ts", "index.tsx", "index.js", "__init__.py"] {
            let candidate = dir_candidate.join(index_name);
            if candidate.is_file() {
                return Some(rel_display(&candidate));
            }
        }
        if dir_candidate.is_dir() {
            let mut subfiles = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&dir_candidate) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() {
                        subfiles.push(rel_display(&p));
                    }
                }
            }
            subfiles.sort();
            subfiles.truncate(3);
            if !subfiles.is_empty() {
                return Some(subfiles.join("`, `"));
            }
        }
        if abs_missing.file_name().and_then(|n| n.to_str()) == Some("mod.rs")
            && let Some(dir_name) = parent.file_name().and_then(|n| n.to_str())
            && let Some(grandparent) = parent.parent()
        {
            let flat_rs = grandparent.join(format!("{dir_name}.rs"));
            if flat_rs.is_file() {
                return Some(rel_display(&flat_rs));
            }
        }
        // Check siblings in the same existing parent directory for small typos (distance <= 3).
        if parent.is_dir()
            && let Some(missing_name) = abs_missing.file_name().and_then(|n| n.to_str())
            && let Ok(entries) = std::fs::read_dir(parent)
        {
            let mut best_sibling: Option<(usize, PathBuf)> = None;
            for entry in entries.flatten() {
                let p = entry.path();
                let Some(fname) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if p.is_file() {
                    let dist = edit_distance(missing_name, fname);
                    if dist <= 3 && best_sibling.as_ref().is_none_or(|(b, _)| dist < *b) {
                        best_sibling = Some((dist, p));
                    }
                }
            }
            if let Some((_, best)) = best_sibling {
                return Some(rel_display(&best));
            }
        }
    }

    // 2. Workspace scan for files with the same filename or `<stem>/mod.rs` elsewhere in `root`.
    let target_name = abs_missing.file_name().and_then(|n| n.to_str())?;
    let target_stem = abs_missing.file_stem().and_then(|s| s.to_str())?;
    let missing_rel = abs_missing.strip_prefix(root).unwrap_or(&abs_missing);
    let missing_components: Vec<&str> = missing_rel
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    let mut scanned = 0usize;
    let mut candidates: Vec<(usize, usize, String)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if scanned >= MAX_SUGGEST_SCAN_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if p.is_dir() {
                if !SKIPPED_SUGGEST_DIRS.contains(&name) && !name.starts_with('.') {
                    stack.push(p);
                }
                continue;
            }
            scanned += 1;
            if scanned > MAX_SUGGEST_SCAN_FILES {
                break;
            }
            let is_exact_name = name == target_name;
            let is_mod_split = name == "mod.rs"
                && p.parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|n| n.to_str())
                    == Some(target_stem);
            if !is_exact_name && !is_mod_split {
                continue;
            }
            let rel = p.strip_prefix(root).unwrap_or(&p);
            let comp_overlap = rel
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .filter(|c| missing_components.contains(c))
                .count();
            let priority = if is_exact_name { 0 } else { 1 };
            candidates.push((priority, usize::MAX - comp_overlap, rel_display(&p)));
        }
    }
    candidates.sort();
    candidates.dedup_by(|a, b| a.2 == b.2);
    candidates.truncate(3);
    if candidates.is_empty() {
        None
    } else {
        let joined: Vec<String> = candidates.into_iter().map(|(_, _, rel)| rel).collect();
        Some(joined.join("`, `"))
    }
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b_chars.len()).collect();
    let mut curr = vec![0usize; b_chars.len() + 1];
    for (i, &ca) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b_chars.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_chars.len()]
}
