/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Explicit checkout-relative `[sync] include` rules from `prod-code.toml` (#1836).
//!
//! Ordinary sync skips output and data trees (`site/data/`, `results/`, `data/`, …) and, in
//! non-Git workspaces, extensions outside the source/manifest allowlist. `[sync] include` opts
//! required checkout inputs back into upload without overriding ignore files, baseline build/VCS
//! exclusions, symlink containment or per-file size caps.

use crate::config::{is_baseline_ignored, load_config};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
struct IncludePrefix {
    prefix: PathBuf,
    allow_descendants: bool,
}

/// Compiled `[sync] include` matcher for a workspace root.
#[derive(Debug, Clone, Default)]
pub struct SyncInclude {
    matcher: Option<Gitignore>,
    prefixes: Vec<IncludePrefix>,
}

impl SyncInclude {
    /// Load `[sync] include` from global and workspace `prod-code.toml` for `root`.
    pub fn load(root: &Path) -> Self {
        let config = load_config(root);
        Self::from_patterns(root, &config.sync.include)
    }

    /// Compile `patterns` against `root`, discarding empty, escaping, hidden or baseline-ignored
    /// entries.
    pub fn from_patterns(root: &Path, patterns: &[String]) -> Self {
        if patterns.is_empty() {
            return Self::default();
        }
        let mut builder = GitignoreBuilder::new(root);
        let mut prefixes = Vec::new();
        for raw in patterns {
            let Some((anchored, prefix)) = parse_pattern(raw) else {
                continue;
            };
            if builder.add_line(None, &anchored).is_ok() {
                prefixes.push(prefix);
            }
        }
        if prefixes.is_empty() {
            return Self::default();
        }
        let Ok(matcher) = builder.build() else {
            return Self::default();
        };
        Self {
            matcher: Some(matcher),
            prefixes,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.prefixes.is_empty()
    }

    /// Whether checkout-relative file `rel_path` is opted into upload by `[sync] include`.
    pub fn is_included(&self, rel_path: &str) -> bool {
        let Some(matcher) = &self.matcher else {
            return false;
        };
        let rel = Path::new(rel_path);
        if rel.as_os_str().is_empty() || is_non_overridable_sync_path(rel) {
            return false;
        }
        matcher.matched_path_or_any_parents(rel, false).is_ignore()
    }

    /// Whether checkout-relative directory `rel_dir` is an ancestor of, equal to, or inside a
    /// `[sync] include` pattern, so a non-Git walk must descend into it even when `walk_prunes`
    /// would otherwise skip it.
    pub fn may_contain_included(&self, rel_dir: &Path) -> bool {
        if self.prefixes.is_empty()
            || rel_dir.as_os_str().is_empty()
            || is_non_overridable_sync_path(rel_dir)
        {
            return false;
        }
        self.prefixes.iter().any(|rule| {
            rule.prefix.as_os_str().is_empty()
                || rule.prefix.starts_with(rel_dir)
                || (rule.allow_descendants && rel_dir.starts_with(&rule.prefix))
        })
    }
}

/// Paths that `[sync] include` and sync omission warnings never treat as uploadable checkout
/// inputs: baseline build/dependency/VCS trees, hidden dot-paths and Python bytecode caches.
pub(crate) fn is_non_overridable_sync_path(rel: &Path) -> bool {
    if is_baseline_ignored(rel) {
        return true;
    }
    rel.components().any(|comp| match comp {
        Component::Normal(s) => {
            let name = s.to_string_lossy();
            name.starts_with('.') || name == "__pycache__"
        }
        _ => true,
    })
}

fn parse_pattern(raw: &str) -> Option<(String, IncludePrefix)> {
    let mut trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with(['/', '\\', '!']) || trimmed.contains('\\') {
        return None;
    }
    while let Some(rest) = trimmed.strip_prefix("./") {
        trimmed = rest;
    }
    let trailing_slash = trimmed.ends_with('/');
    let clean = trimmed.trim_end_matches('/');
    if clean.is_empty() {
        return None;
    }

    let segments: Vec<&str> = clean.split('/').collect();
    if segments.first().is_some_and(|first| first.contains(':')) {
        return None;
    }
    for &seg in &segments {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if !has_glob_meta(seg)
            && (seg.starts_with('.') || seg == "__pycache__" || is_baseline_ignored(Path::new(seg)))
        {
            return None;
        }
    }

    let literal_len = segments
        .iter()
        .take_while(|&&seg| !has_glob_meta(seg))
        .count();
    let mut prefix = PathBuf::new();
    for &seg in &segments[..literal_len] {
        prefix.push(seg);
    }
    let rest = &segments[literal_len..];
    let allow_descendants = rest.is_empty() || rest.len() > 1 || rest[0].contains("**");

    let anchored = if trailing_slash {
        format!("/{clean}/")
    } else {
        format!("/{clean}")
    };
    Some((
        anchored,
        IncludePrefix {
            prefix,
            allow_descendants,
        },
    ))
}

fn has_glob_meta(segment: &str) -> bool {
    segment.contains(['*', '?', '[', '{'])
}
