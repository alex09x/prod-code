/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Where the old name is still written after a Swift rename (#1953). sourcekit-lsp renames what
//! its index knows; with no index of the target built, a call in another file is not known and
//! stays, while the rename reports success. The files the edit left alone are read for the old
//! name, and the rename says which still write it.

use std::path::{Path, PathBuf};

use crate::tools::symbols::nested_projects::{MAX_SCANNED_FILES, source_files};

/// The most places listed; the count says how many there are in all.
const LISTED: usize = 8;

/// The identifier at the 1-based `line` and `character` of `text`: the name a rename at that
/// position replaces.
pub(crate) fn old_name_at(text: &str, line: u32, character: u32) -> Option<String> {
    let at = crate::signature::offset_of(text, line, character)?;
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at].rfind(|c: char| !is_word(c)).map_or(0, |i| i + 1);
    let name: String = text[start..].chars().take_while(|c| is_word(*c)).collect();
    (!name.is_empty()).then_some(name)
}

/// Whole-word occurrences of `name` in `text`, as 1-based lines, outside `//` comment lines.
fn lines_naming(text: &str, name: &str) -> Vec<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| {
            line.match_indices(name).any(|(at, _)| {
                let before = line[..at].chars().next_back();
                let after = line[at + name.len()..].chars().next();
                !before.is_some_and(is_word) && !after.is_some_and(is_word)
            })
        })
        .map(|(index, _)| index + 1)
        .collect()
}

/// The Swift files under `root` that the rename did not edit and that still write `old`,
/// with the first line of each.
pub(crate) fn swift_files_still_naming(
    root: &Path,
    old: &str,
    edited: &[PathBuf],
) -> Vec<(PathBuf, usize)> {
    if old.is_empty() {
        return Vec::new();
    }
    let edited: Vec<PathBuf> = edited
        .iter()
        .map(|path| std::fs::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect();
    source_files(root)
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "swift")
        })
        .take(MAX_SCANNED_FILES)
        .filter(|path| {
            let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            !edited.contains(&path)
        })
        .filter_map(|path| {
            let text = crate::tools::symbols::across_projects::read_name_scan_text(&path)?;
            let first = *lines_naming(&text, old).first()?;
            Some((path, first))
        })
        .collect()
}

/// The sentence the rename adds when `leftovers` is not empty.
pub(crate) fn render(root: &Path, old: &str, leftovers: &[(PathBuf, usize)]) -> Option<String> {
    if leftovers.is_empty() {
        return None;
    }
    let mut text = format!(
        "\n\nwarning: `{old}` is still written in {} Swift file(s) the rename did not edit. \
         sourcekit-lsp renames what its index knows, and a call in another file is not known \
         until the target is built; build it and run the rename again, or edit these (some may \
         name a different symbol):",
        leftovers.len()
    );
    for (path, line) in leftovers.iter().take(LISTED) {
        let shown = path.strip_prefix(root).unwrap_or(path).display();
        text.push_str(&format!("\n  {shown}:{line}"));
    }
    if leftovers.len() > LISTED {
        text.push_str(&format!("\n  ... and {} more", leftovers.len() - LISTED));
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_whole_words_outside_comment_lines_count() {
        let source = "// check() is documented here\nlet a = model.check()\nlet b = checked()\nlet c = recheck()\nfunc check_all() {}\n";
        assert_eq!(lines_naming(source, "check"), vec![2]);
    }

    #[test]
    fn files_the_rename_edited_are_not_leftovers_and_others_are_listed() {
        let root = tempfile::tempdir().unwrap();
        let write = |name: &str, text: &str| {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            path
        };
        let edited = write("Sources/Model.swift", "func checkFolderAccess() {}\n");
        let caller = write(
            "Sources/Step.swift",
            "struct Step {\n  func run() {\n    model.checkSelectedFolder()\n  }\n}\n",
        );
        write("Sources/Other.swift", "func unrelated() {}\n");
        write("Sources/Docs.swift", "// checkSelectedFolder was renamed\n");
        write("Sources/Note.md", "checkSelectedFolder\n");

        let leftovers =
            swift_files_still_naming(root.path(), "checkSelectedFolder", &[edited.clone()]);
        assert_eq!(leftovers.len(), 1, "{leftovers:?}");
        assert_eq!(leftovers[0].0, caller);
        assert_eq!(leftovers[0].1, 3);

        let text = render(root.path(), "checkSelectedFolder", &leftovers).unwrap();
        assert!(text.contains("Sources/Step.swift:3"), "{text}");
        assert!(text.contains("build it and run the rename again"), "{text}");
        assert_eq!(render(root.path(), "checkSelectedFolder", &[]), None);
    }
}
