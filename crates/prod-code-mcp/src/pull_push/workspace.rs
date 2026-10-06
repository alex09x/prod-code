/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::pull_push::languages::parse_classes_in_text;
use crate::pull_push::types::ClassDecl;
use std::path::{Path, PathBuf};

/// Search workspace for a class by name and language.
pub fn find_class_in_workspace(
    root: &Path,
    class_name: &str,
    language: &str,
) -> Option<(PathBuf, String, ClassDecl)> {
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let p = entry.path();
        let lang = crate::lang::language_id_for_path(p);
        if lang != language {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(p) {
            let classes = parse_classes_in_text(&content, language, p);
            if let Some(c) = classes.into_iter().find(|cls| cls.name == class_name) {
                return Some((p.to_path_buf(), content, c));
            }
        }
    }
    None
}

/// Search workspace for all subclasses of a given superclass.
pub fn find_subclasses_in_workspace(
    root: &Path,
    super_class_name: &str,
    language: &str,
) -> Vec<(PathBuf, String, ClassDecl)> {
    let mut results = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .build()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let p = entry.path();
        let lang = crate::lang::language_id_for_path(p);
        if lang != language {
            continue;
        }
        if let Ok(content) = std::fs::read_to_string(p) {
            let classes = parse_classes_in_text(&content, language, p);
            for cls in classes {
                if cls.super_names.iter().any(|s| s == super_class_name) {
                    results.push((p.to_path_buf(), content.clone(), cls));
                }
            }
        }
    }
    results
}
