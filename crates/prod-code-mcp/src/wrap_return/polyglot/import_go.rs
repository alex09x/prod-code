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

fn module_info(path: &Path) -> Option<(PathBuf, String)> {
    let mut dir = path.parent()?;
    loop {
        let manifest = dir.join("go.mod");
        if let Ok(contents) = std::fs::read_to_string(manifest) {
            let module = contents.lines().find_map(|line| {
                line.split("//")
                    .next()
                    .unwrap_or("")
                    .trim()
                    .strip_prefix("module ")
                    .map(|module| module.trim().trim_matches('"').to_string())
            })?;
            return Some((dir.to_path_buf(), module));
        }
        dir = dir.parent()?;
    }
}

fn target_import_path(decl_file: &Path) -> Option<(String, String)> {
    let (root, module) = module_info(decl_file)?;
    let relative = decl_file
        .parent()?
        .strip_prefix(&root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let import_path = if relative.is_empty() {
        module
    } else {
        format!("{module}/{relative}")
    };
    let package = std::fs::read_to_string(decl_file)
        .ok()?
        .lines()
        .find_map(|line| {
            line.split("//")
                .next()
                .unwrap_or("")
                .trim()
                .strip_prefix("package ")
                .map(str::to_string)
        })?;
    Some((import_path, package))
}

fn import_alias(spec: &str, expected: &str, default_alias: &str) -> Option<String> {
    let mut parts = spec.split_whitespace();
    let first = parts.next()?;
    let (alias, path) = if first.starts_with('"') || first.starts_with('`') {
        (default_alias, first)
    } else {
        (first, parts.next()?)
    };
    let path = path.trim_matches('"').trim_matches('`');
    if path != expected || alias == "_" {
        return None;
    }
    Some(alias.to_string())
}

fn imported_package_aliases(content: &str, caller_path: &Path, decl_file: &Path) -> Vec<String> {
    let Some((decl_root, module)) = module_info(decl_file) else {
        return Vec::new();
    };
    let Some((caller_root, caller_module)) = module_info(caller_path) else {
        return Vec::new();
    };
    if caller_root != decl_root || caller_module != module {
        return Vec::new();
    }
    let Some((expected, default_alias)) = target_import_path(decl_file) else {
        return Vec::new();
    };

    let mut aliases = Vec::new();
    let mut in_block = false;
    for line in content.lines() {
        let line = line.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if in_block {
            if line == ")" {
                in_block = false;
            } else if let Some(alias) = import_alias(line, &expected, &default_alias) {
                aliases.push(alias);
            }
        } else if let Some(spec) = line.strip_prefix("import ") {
            let spec = spec.trim();
            if spec == "(" {
                in_block = true;
            } else if let Some(alias) = import_alias(spec, &expected, &default_alias) {
                aliases.push(alias);
            }
        }
    }
    aliases
}

pub(crate) fn go_imported_symbols(
    content: &str,
    caller_path: &Path,
    decl_file: &Path,
    fn_name: &str,
) -> Vec<String> {
    if imported_package_aliases(content, caller_path, decl_file).is_empty() {
        Vec::new()
    } else {
        vec![fn_name.to_string()]
    }
}

pub(crate) fn is_go_namespace_import(
    content: &str,
    receiver: &str,
    caller_path: &Path,
    decl_file: &Path,
) -> bool {
    imported_package_aliases(content, caller_path, decl_file)
        .iter()
        .any(|alias| alias == receiver)
}
