/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::WatchConfig;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};

/// Immutable baseline directory and file names that are always ignored.
/// Checked before regex/glob matching for zero-allocation fast filtering.
pub const BASELINE_IGNORES: &[&str] = &[
    ".git",
    ".idea",
    ".vscode",
    ".codex",
    ".gemini",
    ".claude",
    "target",
    "node_modules",
    ".venv",
    "DerivedData",
    ".prod-code-last-used",
    ".prod-code",
    ".DS_Store",
];

/// Fast check for baseline non-overridable ignores.
pub fn is_baseline_ignored(rel: &Path) -> bool {
    rel.components().any(|c| {
        let s = c.as_os_str();
        BASELINE_IGNORES.iter().any(|&b| s == b)
    })
}

/// Checks if a relative path corresponds to a configuration or ignore definition file
/// that should trigger an ignore matcher rebuild.
pub fn is_config_or_ignore_file(rel: &Path) -> bool {
    rel.file_name()
        .and_then(|n| n.to_str())
        .map(|name| {
            matches!(
                name,
                ".gitignore" | ".ignore" | ".prod-code.toml" | "prod-code.toml" | "exclude"
            )
        })
        .unwrap_or(false)
}

/// An immutable, thread-safe snapshot of ignore rules for a workspace.
pub struct IgnoreSnapshot {
    root: PathBuf,
    gitignore: Gitignore,
}

impl IgnoreSnapshot {
    /// Builds an `IgnoreSnapshot` for `root` with the given `WatchConfig`.
    pub fn build(root: &Path, config: &WatchConfig) -> Self {
        let mut builder = GitignoreBuilder::new(root);

        if config.use_gitignore {
            let root_gi = root.join(".gitignore");
            if root_gi.is_file() {
                let _ = builder.add(&root_gi);
            }
            let root_ig = root.join(".ignore");
            if root_ig.is_file() {
                let _ = builder.add(&root_ig);
            }
            let git_exclude = root.join(".git").join("info").join("exclude");
            if git_exclude.is_file() {
                let _ = builder.add(&git_exclude);
            }
        }

        for pattern in &config.ignore {
            let _ = builder.add_line(None, pattern);
        }

        let gitignore = builder.build().unwrap_or_else(|_| Gitignore::empty());

        Self {
            root: root.to_path_buf(),
            gitignore,
        }
    }

    /// Tests whether `path` is ignored under this snapshot.
    pub fn is_ignored(&self, path: &Path) -> bool {
        let Ok(rel) = path.strip_prefix(&self.root) else {
            return false;
        };

        // 1. Fast-path baseline check
        if is_baseline_ignored(rel) {
            return true;
        }

        // 2. Gitignore / user pattern check
        let is_dir = path.is_dir();
        self.gitignore
            .matched_path_or_any_parents(rel, is_dir)
            .is_ignore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn baseline_ignores_cover_vcs_and_agent_directories() {
        assert!(is_baseline_ignored(Path::new(".git/HEAD")));
        assert!(is_baseline_ignored(Path::new(".git/index")));
        assert!(is_baseline_ignored(Path::new(".idea/workspace.xml")));
        assert!(is_baseline_ignored(Path::new(".vscode/settings.json")));
        assert!(is_baseline_ignored(Path::new(
            ".gemini/antigravity-cli/log"
        )));
        assert!(is_baseline_ignored(Path::new(".claude/session.json")));
        assert!(is_baseline_ignored(Path::new(".codex/history.json")));
        assert!(is_baseline_ignored(Path::new("target/debug/app")));
        assert!(is_baseline_ignored(Path::new("node_modules/pkg/index.js")));
        assert!(is_baseline_ignored(Path::new(".venv/bin/python")));
        assert!(is_baseline_ignored(Path::new("DerivedData/Build/Products")));
        assert!(is_baseline_ignored(Path::new(".prod-code-last-used")));

        assert!(!is_baseline_ignored(Path::new("src/main.rs")));
        assert!(!is_baseline_ignored(Path::new("Cargo.toml")));
    }

    #[test]
    fn ignore_snapshot_respects_gitignore_and_custom_ignores() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        std::fs::write(root.join(".gitignore"), "dist/\n*.tmp\n").unwrap();

        let mut config = WatchConfig::default();
        config.ignore.push("fixtures/extra".to_string());

        let snapshot = IgnoreSnapshot::build(root, &config);

        assert!(snapshot.is_ignored(&root.join("dist").join("bundle.js")));
        assert!(snapshot.is_ignored(&root.join("foo.tmp")));
        assert!(snapshot.is_ignored(&root.join("fixtures").join("extra").join("sample.txt")));
        assert!(!snapshot.is_ignored(&root.join("src").join("lib.rs")));
    }
}
