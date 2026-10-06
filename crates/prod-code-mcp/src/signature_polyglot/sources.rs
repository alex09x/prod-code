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

use crate::parameter_object::Language;

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
    }
}

pub fn is_c_cpp_prototype(content: &str, at: usize, _args_start: usize, args_end: usize) -> bool {
    let after_paren = content[args_end + 1..].trim_start();
    if !after_paren.starts_with(';') {
        return false;
    }

    let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
    let before = content[line_start..at].trim();

    // If there is nothing before fn_name on this line, in C/C++ it cannot be a prototype
    // (a prototype must have a return type like `void foo();` or `int foo();`).
    if before.is_empty() {
        return false;
    }

    let last_word = before.split_whitespace().last().unwrap_or("");
    if matches!(
        last_word,
        "return" | "throw" | "case" | "sizeof" | "decltype" | "co_return" | "co_yield"
    ) {
        return false;
    }
    if before.ends_with('=')
        || before.ends_with('(')
        || before.ends_with('[')
        || before.ends_with(',')
        || before.ends_with('?')
        || before.ends_with(':')
        || before.ends_with('!')
        || before.ends_with('+')
        || before.ends_with('-')
        || before.ends_with('*')
        || before.ends_with('/')
        || before.ends_with('%')
        || before.ends_with('&')
        || before.ends_with('|')
        || before.ends_with('^')
    {
        return false;
    }

    let first_word = before.split_whitespace().next().unwrap_or("");
    if matches!(first_word, "if" | "while" | "for" | "switch" | "catch") {
        return false;
    }

    true
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
