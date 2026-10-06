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

use super::go_imports::get_go_package;

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

pub(crate) fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

pub(crate) fn display_relative_or_name(from_file: &Path, to_file: &Path) -> String {
    let from_dir = from_file.parent().unwrap_or(Path::new(""));
    let rel = path_relative_from(to_file, from_dir);
    let s = rel.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        to_file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        s
    }
}

pub(crate) fn module_display_name(file: &Path, root: &Path, lang: Language) -> String {
    match lang {
        Language::Python => python_module_specifier(file, file, root),
        Language::Go => get_go_package(file),
        _ => display(root, file),
    }
}

pub fn is_compatible_language_family(a: Language, b: Language) -> bool {
    if a == b {
        return true;
    }
    matches!(
        (a, b),
        (Language::TypeScript, Language::JavaScript)
            | (Language::JavaScript, Language::TypeScript)
            | (Language::Cpp, Language::C)
            | (Language::C, Language::Cpp)
    )
}

pub fn path_relative_from(path: &Path, base: &Path) -> PathBuf {
    let path_comps: Vec<_> = path.components().collect();
    let base_comps: Vec<_> = base.components().collect();

    let mut common = 0;
    while common < path_comps.len()
        && common < base_comps.len()
        && path_comps[common] == base_comps[common]
    {
        common += 1;
    }

    let mut result = PathBuf::new();
    for _ in common..base_comps.len() {
        result.push("..");
    }
    for comp in &path_comps[common..] {
        result.push(comp.as_os_str());
    }
    if result.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        result
    }
}

pub fn relative_import_specifier(from_file: &Path, to_file: &Path) -> String {
    let from_dir = from_file.parent().unwrap_or(Path::new(""));
    let mut to_str = to_file.to_string_lossy().into_owned();
    for ext in &[".d.ts", ".tsx", ".ts", ".jsx", ".js"] {
        if let Some(stripped) = to_str.strip_suffix(ext) {
            to_str = stripped.to_string();
            break;
        }
    }
    let to_path = PathBuf::from(&to_str);
    let rel = path_relative_from(&to_path, from_dir);
    let mut s = rel.to_string_lossy().replace('\\', "/");
    if !s.starts_with("./") && !s.starts_with("../") {
        s = format!("./{s}");
    }
    s
}

pub fn python_module_specifier(from_file: &Path, to_file: &Path, root: &Path) -> String {
    let _ = from_file;
    let rel = to_file.strip_prefix(root).unwrap_or(to_file);
    let mut s = rel.to_string_lossy().into_owned();
    if let Some(stripped) = s.strip_suffix(".py") {
        s = stripped.to_string();
    }
    if let Some(stripped) = s.strip_suffix("/__init__") {
        s = stripped.to_string();
    }
    s.replace(['/', '\\'], ".")
}

pub fn is_symbol_used(content: &str, symbol: &str) -> bool {
    for (at, _) in content.match_indices(symbol) {
        if at > 0 {
            let prev = content[..at].chars().next_back().unwrap();
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
        }
        let after = &content[at + symbol.len()..];
        if after.starts_with(|c: char| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let line_start = content[..at].rfind('\n').map_or(0, |p| p + 1);
        let before_on_line = content[line_start..at].trim_start();
        if before_on_line.starts_with("//")
            || before_on_line.starts_with('#')
            || before_on_line.starts_with('*')
        {
            continue;
        }
        return true;
    }
    false
}
