/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Go source that Git ignores is not synced to the build node, so neither gopls nor the compiler
//! there sees it. A caller in such a file is invisible to every check a deletion relies on, so
//! a deletion whose name it mentions is refused before any evidence is gathered (#1397).

use std::path::{Path, PathBuf};

use anyhow::Result;

/// The untracked `.go` files of `file`'s package directory that Git ignores. Empty outside a Git
/// checkout, where nothing is filtered from the sync.
fn ignored_package_sources(file: &Path) -> Vec<PathBuf> {
    let Some(dir) = file.parent() else {
        return Vec::new();
    };
    let Ok(out) = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
            "--",
            ".",
        ])
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    out.stdout
        .split(|b| *b == 0)
        .filter_map(|rel| std::str::from_utf8(rel).ok())
        // The package is this directory; a subdirectory is another package.
        .filter(|rel| rel.ends_with(".go") && !rel.contains('/'))
        .map(|rel| dir.join(rel))
        .collect()
}

/// Whether `text` holds `name` as a whole word.
fn mentions(text: &str, name: &str) -> bool {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        !text[..at].chars().next_back().is_some_and(ident)
            && !text[at + name.len()..].chars().next().is_some_and(ident)
    })
}

/// The word of `text` at byte `at`, if `at` is inside one.
pub(crate) fn word_at(text: &str, at: usize) -> Option<&str> {
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    if !text.get(at..)?.starts_with(ident) {
        return None;
    }
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .find(|(_, c)| !ident(*c))
        .map_or(text.len(), |(i, _)| at + i);
    (start < end).then(|| &text[start..end])
}

/// Refuses when a Git-ignored Go file of the declaring package mentions `name`.
pub(crate) fn refuse_ignored_callers(file: &Path, name: &str) -> Result<()> {
    for source in ignored_package_sources(file) {
        let Ok(text) = std::fs::read_to_string(&source) else {
            continue;
        };
        anyhow::ensure!(
            !mentions(&text, name),
            "{} mentions {name} but Git ignores it, so it is not synced and neither gopls nor \
             the compiler sees it; deletion is refused",
            source.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    #[test]
    fn an_ignored_caller_in_the_package_refuses_and_others_do_not() {
        let dir = tempfile::Builder::new().prefix("goign").tempdir().unwrap();
        let root = dir.path();
        git(root, &["init", "-q"]);
        std::fs::write(root.join(".gitignore"), "generated.go\nsub/\n").unwrap();
        std::fs::write(root.join("main.go"), "package main\n\nfunc helper() {}\n").unwrap();
        std::fs::write(
            root.join("generated.go"),
            "package main\n\nfunc use() { helper() }\n",
        )
        .unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(
            root.join("sub/other.go"),
            "package sub\n\nfunc helper() {}\n",
        )
        .unwrap();
        let main = root.join("main.go");
        let error = refuse_ignored_callers(&main, "helper")
            .unwrap_err()
            .to_string();
        assert!(error.contains("generated.go mentions helper"), "{error}");
        // A name the ignored file does not mention, as a whole word, is not refused.
        assert!(refuse_ignored_callers(&main, "help").is_ok());
        assert!(refuse_ignored_callers(&main, "other").is_ok());
        // Tracked files are synced even when a pattern matches them.
        git(root, &["add", "-f", "generated.go"]);
        assert!(refuse_ignored_callers(&main, "helper").is_ok());
        let text = "func helper() {}";
        assert_eq!(word_at(text, 7), Some("helper"));
        assert_eq!(word_at(text, 5), Some("helper"));
        assert_eq!(word_at(text, 4), None);
    }

    #[test]
    fn outside_git_nothing_is_ignored() {
        let dir = tempfile::Builder::new()
            .prefix("goplain")
            .tempdir()
            .unwrap();
        std::fs::write(dir.path().join("a.go"), "package a\nfunc helper() {}\n").unwrap();
        assert!(refuse_ignored_callers(&dir.path().join("a.go"), "helper").is_ok());
    }
}
