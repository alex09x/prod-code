/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Rust,
    Python,
    TypeScript,
    JavaScript,
    Go,
    C,
    Cpp,
    Swift,
    Java,
    Kotlin,
    Csharp,
    Zig,
}

impl Language {
    pub fn of(file: &Path) -> Option<Self> {
        let ext = file.extension().and_then(|e| e.to_str())?;
        match ext {
            "rs" => Some(Self::Rust),
            "py" => Some(Self::Python),
            "ts" | "tsx" => Some(Self::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Self::JavaScript),
            "go" => Some(Self::Go),
            "c" | "h" => Some(Self::C),
            "cpp" | "cc" | "cxx" | "hpp" => Some(Self::Cpp),
            "swift" => Some(Self::Swift),
            "java" => Some(Self::Java),
            "kt" | "kts" => Some(Self::Kotlin),
            "cs" => Some(Self::Csharp),
            "zig" | "zon" => Some(Self::Zig),
            _ => None,
        }
    }
}

pub fn is_candidate_source_file(path: &Path, lang: Language) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    match lang {
        Language::TypeScript => matches!(ext, "ts" | "tsx" | "js" | "jsx"),
        Language::JavaScript => matches!(ext, "js" | "jsx" | "ts" | "tsx"),
        Language::Python => ext == "py",
        Language::Cpp | Language::C => {
            matches!(ext, "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" | "hxx")
        }
        Language::Swift => ext == "swift",
        Language::Go => ext == "go",
        Language::Rust => ext == "rs",
        Language::Java => ext == "java",
        Language::Kotlin => matches!(ext, "kt" | "kts"),
        Language::Csharp => ext == "cs",
        Language::Zig => matches!(ext, "zig" | "zon"),
    }
}

pub fn collect_workspace_sources(root: &Path, lang: Language) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with('.')
                || name_str == "target"
                || name_str == "node_modules"
                || name_str == "build"
                || name_str == ".build"
                || name_str == "dist"
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && is_candidate_source_file(&path, lang) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

pub fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Whether `text` holds `word` as an isolated identifier.
pub fn mentions(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(i, _)| {
        !text[..i].chars().next_back().is_some_and(is_ident)
            && !text[i + word.len()..].chars().next().is_some_and(is_ident)
    })
}
