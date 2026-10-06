/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

use anyhow::Result;

use crate::parameter_object::Language;

use super::specifiers::is_ident;

pub(crate) fn go_import_package(spec: &str) -> Option<String> {
    let spec = spec
        .split_once("//")
        .map_or(spec, |(before, _)| before)
        .trim();
    let words: Vec<&str> = spec.split_whitespace().collect();
    let (alias, path) = match words.as_slice() {
        [path] => (None, *path),
        [alias, path] if is_ident(alias.chars().next()?) && alias.chars().all(is_ident) => {
            (Some(*alias), *path)
        }
        _ => return None,
    };
    if matches!(alias, Some("." | "_")) || !path.starts_with('"') || !path.ends_with('"') {
        return None;
    }
    let import_path = &path[1..path.len() - 1];
    if import_path == "C" {
        return None;
    }
    let package = match alias {
        Some(alias) => alias.to_string(),
        None if !import_path.contains('/') => import_path.to_string(),
        // Without an explicit alias, a path's last component need not be its package name.
        None => return None,
    };
    package.chars().all(is_ident).then_some(package)
}

pub(crate) fn go_package_is_used(content: &str, package: &str) -> bool {
    content.match_indices(package).any(|(at, _)| {
        let end = at + package.len();
        !(at > 0 && content[..at].chars().next_back().is_some_and(is_ident))
            && !content[end..].chars().next().is_some_and(is_ident)
            && !crate::inline_parameter::is_in_comment(content, at, Language::Go)
            && !crate::inline_parameter::is_in_string(content, at, Language::Go)
            && content[end..].trim_start().starts_with('.')
    })
}

pub(crate) fn remove_unused_go_imports(source: &str) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let mut output = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "import (" {
            let Some(close) = (i + 1..lines.len()).find(|&at| lines[at].trim() == ")") else {
                output.push(lines[i].to_string());
                i += 1;
                continue;
            };
            let specs: Option<Vec<(&str, String)>> = lines[i + 1..close]
                .iter()
                .map(|line| go_import_package(line).map(|package| (*line, package)))
                .collect();
            if let Some(specs) = specs {
                let retained: Vec<_> = specs
                    .into_iter()
                    .filter(|(_, package)| go_package_is_used(source, package))
                    .collect();
                if !retained.is_empty() {
                    output.push(lines[i].to_string());
                    output.extend(retained.into_iter().map(|(line, _)| line.to_string()));
                    output.push(lines[close].to_string());
                }
                i = close + 1;
                continue;
            }
        }
        if let Some(spec) = lines[i].trim().strip_prefix("import ")
            && let Some(package) = go_import_package(spec)
            && !go_package_is_used(source, &package)
        {
            i += 1;
            continue;
        }
        output.push(lines[i].to_string());
        i += 1;
    }
    let mut result = output.join("\n");
    if source.ends_with('\n') {
        result.push('\n');
    }
    result
}

pub(crate) fn get_go_package(file: &Path) -> String {
    if let Ok(content) = std::fs::read_to_string(file) {
        for line in content.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("package ") {
                let name = rest.trim();
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    file.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string())
}

pub(crate) fn go_module_import_path(target_dir: &Path) -> Result<String> {
    let target = std::fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());
    let mut directory = Some(target.as_path());
    while let Some(dir) = directory {
        if let Ok(manifest) = std::fs::read_to_string(dir.join("go.mod"))
            && let Some(module) = manifest.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("module ")
                    .map(|value| value.trim().trim_matches('"').to_string())
                    .filter(|value| !value.is_empty())
            })
        {
            let relative = target.strip_prefix(dir).unwrap_or(Path::new(""));
            let suffix = relative.to_string_lossy().replace('\\', "/");
            return Ok(if suffix.is_empty() {
                module
            } else {
                format!("{module}/{suffix}")
            });
        }
        directory = dir.parent();
    }
    anyhow::bail!(
        "cannot find the Go module path for {}",
        target_dir.display()
    )
}

pub(crate) fn rewrite_go_call_sites(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
) -> Result<(String, usize)> {
    let mut edits = Vec::new();
    for (at, _) in source_text.match_indices(decl_name) {
        if (at > 0 && source_text[..at].chars().next_back().is_some_and(is_ident))
            || source_text[at + decl_name.len()..]
                .chars()
                .next()
                .is_some_and(is_ident)
            || crate::inline_parameter::is_in_comment(source_text, at, Language::Go)
            || crate::inline_parameter::is_in_string(source_text, at, Language::Go)
        {
            continue;
        }
        let before = source_text[..at].trim_end();
        if before.ends_with('.') {
            continue; // A selector, which may name an unrelated method.
        }
        let after = source_text[at + decl_name.len()..].trim_start();
        if after.starts_with('(') {
            let line_start = source_text[..at].rfind('\n').map_or(0, |i| i + 1);
            if source_text[line_start..at]
                .trim_start()
                .starts_with("func ")
            {
                continue; // A separate declaration is not a call site.
            }
            edits.push((at, decl_name.len()));
        } else {
            anyhow::bail!(
                "Go reference `{decl_name}` at byte {at} is not a call; cannot safely qualify it"
            );
        }
    }
    let count = edits.len();
    let mut rewritten = source_text.to_string();
    for (at, len) in edits.into_iter().rev() {
        rewritten.replace_range(at..at + len, &format!("{target_pkg}.{decl_name}"));
    }
    Ok((rewritten, count))
}

pub(crate) fn add_go_import(content: &str, pkg: &str) -> String {
    let import_str = format!("import \"{pkg}\"\n");
    if let Some(pos) = content.find("package ") {
        let after_pkg = content[pos..].find('\n').map_or(pos + 8, |p| pos + p + 1);
        let mut out = String::new();
        out.push_str(&content[..after_pkg]);
        out.push('\n');
        out.push_str(&import_str);
        out.push_str(&content[after_pkg..]);
        out
    } else {
        format!("{import_str}\n{content}")
    }
}

pub(crate) fn rewrite_go_cross_pkg_in_source(
    source_text: &str,
    decl_name: &str,
    target_pkg: &str,
    target_file: &Path,
    root: &Path,
) -> Result<(String, String)> {
    let target_dir = target_file.parent().unwrap_or(root);
    let target_import_path = go_module_import_path(target_dir)?;
    anyhow::ensure!(
        !source_text.contains(&format!("\"{target_import_path}\"")),
        "the target Go module is already imported and its local alias cannot be safely resolved"
    );
    let (replaced, calls) = rewrite_go_call_sites(source_text, decl_name, target_pkg)?;
    if calls == 0 {
        return Ok((source_text.to_string(), String::new()));
    }
    let with_import = add_go_import(&replaced, &target_import_path);
    Ok((with_import, format!("imported \"{target_import_path}\"")))
}
