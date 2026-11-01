/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Lexical declaration-line matcher (`declared_at`) and bounded local declaration fallback when a
//! primary `workspace/symbol` call reaches its timeout budget (#1118).

use super::across_projects::read_name_scan_text;
use super::types::SymbolHit;
use super::unindexed::collect_candidate_files;
use std::path::Path;

const KEYWORDS: &[&str] = &[
    "fn",
    "struct",
    "enum",
    "trait",
    "type",
    "union",
    "mod",
    "const",
    "static",
    "macro_rules!",
    "func",
    "function",
    "class",
    "interface",
    "def",
    "protocol",
    "actor",
    "extension",
    "typealias",
    "var",
    "let",
    "val",
];

/// The declarations of `name` in `text`, the file at `path`, as (0-based line, 0-based char
/// column), read with comments and string literals blanked out: `// fn Ghost()` and
/// `"fn Ghost()"` declare nothing (#1421).
pub(crate) fn code_declarations(text: &str, path: &Path, name: &str) -> Vec<(usize, usize)> {
    use crate::tools::references::across::{mask_executable_code, masked_code_string};
    // Blanking keeps every byte and line break where it was.
    let code = masked_code_string(text, &mask_executable_code(text, path));
    code.lines()
        .zip(text.lines())
        .enumerate()
        .filter_map(|(index, (code_line, line))| {
            let col = declared_at(code_line, name)?;
            let byte = code_line
                .char_indices()
                .nth(col)
                .map_or(code_line.len(), |(at, _)| at);
            Some((index, line.get(..byte)?.chars().count()))
        })
        .collect()
}

/// The 0-based column where `line` declares `name`: the name right after a declaring keyword
/// (`fn`, `struct`, `func`, `class`, `def`, ...) or after a Go method's receiver
/// (`func (s *T) name`). `None` for a line that only uses it.
pub(crate) fn declared_at(line: &str, name: &str) -> Option<usize> {
    let chars: Vec<char> = line.chars().collect();
    let wanted: Vec<char> = name.chars().collect();
    let is_name = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    (0..chars.len()).find(|&col| {
        if !chars[col..].starts_with(&wanted)
            || (col > 0 && is_name(chars.get(col - 1)))
            || is_name(chars.get(col + wanted.len()))
        {
            return false;
        }
        let before: String = chars[..col].iter().collect();
        let trimmed = before.trim_end();
        if trimmed.len() == before.len() {
            return false;
        }
        let last = trimmed
            .rsplit(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap_or("");
        if KEYWORDS.contains(&last)
            || (trimmed.ends_with(')') && trimmed.trim_start().starts_with("func"))
        {
            return true;
        }
        // Binding modes are part of a Rust pattern, not part of its identifier (#1148).
        let mut words = trimmed.split_whitespace().rev();
        let mut word = words.next();
        if word == Some("mut") {
            word = words.next();
        }
        if word == Some("ref") {
            word = words.next();
        }
        if word == Some("let") {
            return true;
        }
        let after: Vec<char> = chars[col + wanted.len()..]
            .iter()
            .copied()
            .skip_while(|c| c.is_whitespace())
            .collect();
        if after.first() == Some(&':') && after.get(1) != Some(&':') {
            let before_trimmed = trimmed.trim_start();
            if before_trimmed.is_empty()
                || before_trimmed == "pub"
                || before_trimmed.starts_with("pub(")
                || before_trimmed == "mut"
                || before_trimmed == "val"
                || before_trimmed == "var"
                || before_trimmed == "let"
                || before_trimmed == "public"
                || before_trimmed == "private"
                || before_trimmed == "protected"
                || before_trimmed == "readonly"
            {
                return true;
            }
        }
        false
    })
}

/// Scans local source files (including Rust) for exact or prefix declarations matching `name`
/// when `workspace/symbol` reaches its primary timeout budget while an analyzer is still loading
/// or contending on a lock (#1118).
pub(crate) fn scan_timeout_fallback_declarations(
    root: &Path,
    name: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Vec<SymbolHit> {
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return Vec::new();
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let max_hits = limit.clamp(1, 50);
    let mut hits = Vec::new();
    for path in collect_candidate_files(root, hint, true) {
        if std::time::Instant::now() >= deadline {
            break;
        }
        let Some(text) = read_name_scan_text(&path) else {
            continue;
        };
        if !text.contains(name) {
            continue;
        }
        use crate::tools::references::across::{mask_executable_code, masked_code_string};
        let code = masked_code_string(&text, &mask_executable_code(&text, &path));
        for (index, (line, original)) in code.lines().zip(text.lines()).enumerate() {
            if !line.contains(name) {
                continue;
            }
            for (ident, col) in declared_identifiers_on_line(line) {
                if ident.eq_ignore_ascii_case(name)
                    || (name.len() >= 3
                        && ident
                            .to_ascii_lowercase()
                            .starts_with(&name.to_ascii_lowercase()))
                {
                    let byte = line
                        .char_indices()
                        .nth(col)
                        .map_or(line.len(), |(at, _)| at);
                    let Some(prefix) = original.get(..byte) else {
                        continue;
                    };
                    hits.push(SymbolHit {
                        path: path.clone(),
                        name: ident,
                        kind: "Declaration",
                        container: None,
                        line: index as u32 + 1,
                        col: prefix.chars().count() as u32 + 1,
                    });
                    if hits.len() >= max_hits {
                        return hits;
                    }
                }
            }
        }
    }
    hits
}

pub(crate) fn declared_identifiers_on_line(line: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        if chars[i].is_alphabetic() || chars[i] == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let ident: String = chars[start..i].iter().collect();
            if !KEYWORDS.contains(&ident.as_str()) && declared_at(line, &ident) == Some(start) {
                out.push((ident, start));
            }
        } else {
            i += 1;
        }
    }
    out
}

/// Returns `true` when `path` is a standard Cargo target root (`src/lib.rs`, `src/main.rs`,
/// `build.rs`, or `{tests,examples,benches,src/bin}/<name>.rs` or `<name>/main.rs`) under a
/// package directory containing `Cargo.toml` (#1777).
pub(crate) fn is_cargo_target_root(path: &Path) -> bool {
    if path.extension().and_then(|e| e.to_str()) != Some("rs") {
        return false;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if file_name == "build.rs" && parent.join("Cargo.toml").is_file() {
        return true;
    }
    let Some(grandparent) = parent.parent() else {
        return false;
    };
    let parent_name = parent
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if parent_name == "src"
        && matches!(file_name, "lib.rs" | "main.rs")
        && grandparent.join("Cargo.toml").is_file()
    {
        return true;
    }
    if matches!(parent_name, "tests" | "examples" | "benches")
        && grandparent.join("Cargo.toml").is_file()
    {
        return true;
    }
    if let Some(great_grandparent) = grandparent.parent() {
        let gp_name = grandparent
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if parent_name == "bin"
            && gp_name == "src"
            && great_grandparent.join("Cargo.toml").is_file()
        {
            return true;
        }
        if file_name == "main.rs"
            && matches!(gp_name, "tests" | "examples" | "benches")
            && great_grandparent.join("Cargo.toml").is_file()
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn cargo_target_root_detection_recognizes_integration_tests_and_rejects_unlinked_modules() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests")).unwrap();
        let lib = root.join("src/lib.rs");
        let unlinked = root.join("src/orphan.rs");
        let int_test = root.join("tests/lsp_reconnect.rs");
        std::fs::write(&lib, "").unwrap();
        std::fs::write(&unlinked, "").unwrap();
        std::fs::write(&int_test, "").unwrap();

        assert!(is_cargo_target_root(&lib));
        assert!(is_cargo_target_root(&int_test));
        assert!(!is_cargo_target_root(&unlinked));
    }

    #[test]
    fn scan_timeout_fallback_declarations_matches_rust_prefix_and_exact_symbols() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn reserve_for_worktree() {}\npub(crate) fn reserve_memory() {}\nfn unrelated() {}\n",
        )
        .unwrap();

        let hits = scan_timeout_fallback_declarations(root, "reserve_", None, 10);
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["reserve_for_worktree", "reserve_memory"]);
    }

    #[test]
    fn timeout_fallback_ignores_comments_and_string_literals() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(
            root.join("src/lib.rs"),
            "// fn compile_check_comment() {}\nconst FIXTURE: &str = r#\"\npub mod compile_check_string;\n\"#;\n/* 💡 */ pub mod compile_check_real;\n",
        )
        .unwrap();

        let hits = scan_timeout_fallback_declarations(root, "compile_check", None, 10);
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["compile_check_real"]);
        assert_eq!((hits[0].line, hits[0].col), (5, 17));
    }
}
