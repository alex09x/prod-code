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

use anyhow::Result;

use crate::parameter_object::Language;

#[cfg(test)]
pub(crate) use super::go_refs::rewrite_go_call_sites;
pub(crate) use super::go_refs::{
    find_go_type_rhs, is_in_go_string_or_comment, rewrite_go_call_sites_in_context,
};
use super::specifiers::is_ident;

fn parse_go_import_spec(spec: &str) -> Option<(Option<&str>, &str)> {
    let spec = spec
        .split_once("//")
        .map_or(spec, |(before, _)| before)
        .trim();
    let words: Vec<&str> = spec.split_whitespace().collect();
    let (alias, raw_path) = match words.as_slice() {
        [path] => (None, *path),
        [alias, path] if is_ident(alias.chars().next()?) && alias.chars().all(is_ident) => {
            (Some(*alias), *path)
        }
        _ => return None,
    };
    if matches!(alias, Some("." | "_")) || raw_path.len() < 2 {
        return None;
    }
    let import_path = raw_path.trim_matches(['"', '`']);
    (import_path.len() + 2 == raw_path.len() && import_path != "C").then_some((alias, import_path))
}

pub(crate) fn go_import_package(spec: &str) -> Option<String> {
    let (alias, import_path) = parse_go_import_spec(spec)?;
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
            && !is_in_go_string_or_comment(content, at)
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
                let name = rest
                    .split("//")
                    .next()
                    .unwrap_or(rest)
                    .trim()
                    .trim_end_matches(';');
                if !name.is_empty() {
                    return name.to_string();
                }
            }
        }
    }
    let header = super::target::initial_file_header(file, Language::Go);
    for line in header.lines() {
        if let Some(rest) = line.trim().strip_prefix("package ") {
            let name = rest.trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    file.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".to_string())
}

fn find_go_mod(start: &Path) -> Option<(PathBuf, String)> {
    let mut directory = Some(start);
    while let Some(dir) = directory {
        if let Ok(manifest) = std::fs::read_to_string(dir.join("go.mod"))
            && let Some(module) = manifest.lines().find_map(|line| {
                line.trim()
                    .strip_prefix("module ")
                    .map(|v| {
                        v.split("//")
                            .next()
                            .unwrap_or(v)
                            .trim()
                            .trim_matches('"')
                            .to_string()
                    })
                    .filter(|v| !v.is_empty())
            })
        {
            return Some((dir.to_path_buf(), module));
        }
        directory = dir.parent();
    }
    None
}

fn go_files_in_dir(dir: &Path, skip_tests: bool) -> impl Iterator<Item = PathBuf> + use<> {
    std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(move |p| {
            p.extension().is_some_and(|ext| ext == "go")
                && (!skip_tests || !p.to_string_lossy().ends_with("_test.go"))
        })
}

pub(crate) fn go_module_import_path(target_dir: &Path) -> Result<String> {
    let target = std::fs::canonicalize(target_dir).unwrap_or_else(|_| target_dir.to_path_buf());
    if let Some((mod_dir, module)) = find_go_mod(&target) {
        let relative = target.strip_prefix(&mod_dir).unwrap_or(Path::new(""));
        let suffix = relative.to_string_lossy().replace('\\', "/");
        return Ok(if suffix.is_empty() {
            module
        } else {
            format!("{module}/{suffix}")
        });
    }
    anyhow::bail!(
        "cannot find the Go module path for {}",
        target_dir.display()
    )
}

fn resolve_go_import_path_dir(
    import_path: &str,
    source_file: Option<&Path>,
    root: Option<&Path>,
) -> Option<PathBuf> {
    let start = source_file.and_then(Path::parent).or(root)?;
    if let Some((mod_dir, module)) = find_go_mod(start) {
        if import_path == module {
            return Some(mod_dir);
        }
        let candidate = mod_dir.join(import_path.strip_prefix(&format!("{module}/"))?);
        return candidate.is_dir().then_some(candidate);
    }
    let r = root?;
    [r.join(import_path), r.join(import_path.rsplit('/').next()?)]
        .into_iter()
        .find(|cand| cand.is_dir())
}

fn resolve_imported_go_package<'a>(
    source_text: &'a str,
    pkg_alias: &str,
    source_file: Option<&Path>,
    root: Option<&Path>,
) -> Option<(&'a str, Option<PathBuf>)> {
    let mut in_group = false;
    for line in source_text.lines() {
        let trimmed = line.trim();
        let spec = if in_group {
            if trimmed == ")" {
                in_group = false;
                None
            } else {
                parse_go_import_spec(trimmed)
            }
        } else if trimmed == "import (" {
            in_group = true;
            None
        } else {
            trimmed
                .strip_prefix("import ")
                .and_then(parse_go_import_spec)
        };
        let Some((alias, import_path)) = spec else {
            continue;
        };
        if let Some(explicit) = alias {
            if explicit == pkg_alias {
                return Some((
                    import_path,
                    resolve_go_import_path_dir(import_path, source_file, root),
                ));
            }
            continue;
        }
        let dir = resolve_go_import_path_dir(import_path, source_file, root);
        let matches_pkg = match dir.as_deref() {
            Some(d) => {
                go_files_in_dir(d, true).any(|p| get_go_package(&p) == pkg_alias)
                    || import_path.rsplit('/').next() == Some(pkg_alias)
            }
            None => import_path.rsplit('/').next() == Some(pkg_alias),
        };
        if matches_pkg {
            return Some((import_path, dir));
        }
    }
    None
}

fn resolve_type_is_map_or_slice(
    source_text: &str,
    type_expr: &str,
    source_file: Option<&Path>,
    root: Option<&Path>,
    depth: usize,
) -> bool {
    if depth > 4 {
        return false;
    }
    let eval_rhs = |ctx_text: &str, rhs: &str, ctx_file: Option<&Path>| -> bool {
        let rest = rhs.trim_start();
        let rest = rest.strip_prefix('=').map_or(rest, str::trim_start);
        if rest.starts_with("map[") || rest.starts_with('[') {
            return true;
        }
        let len: usize = rest
            .chars()
            .take_while(|&c| is_ident(c) || c == '.')
            .map(char::len_utf8)
            .sum();
        let next = &rest[..len];
        !next.is_empty()
            && !matches!(next, "struct" | "interface" | "func" | "chan")
            && resolve_type_is_map_or_slice(ctx_text, next, ctx_file, root, depth + 1)
    };
    if let Some((pkg_alias, type_name)) = type_expr.split_once('.') {
        if pkg_alias.is_empty()
            || !pkg_alias.chars().all(is_ident)
            || type_name.is_empty()
            || !type_name.chars().all(is_ident)
        {
            return false;
        }
        let Some((import_path, pkg_dir)) =
            resolve_imported_go_package(source_text, pkg_alias, source_file, root)
        else {
            return false;
        };
        if matches!(
            (import_path, type_name),
            ("net/http" | "mime/multipart", "Header")
                | ("net/url", "Values")
                | ("net/textproto", "MIMEHeader")
        ) {
            return true;
        }
        let Some(pkg_dir) = pkg_dir else {
            return false;
        };
        return go_files_in_dir(&pkg_dir, true).any(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| {
                    find_go_type_rhs(&t, type_name).map(|rhs| eval_rhs(&t, rhs, Some(&p)))
                })
                .unwrap_or(false)
        });
    }
    if type_expr.is_empty() || !type_expr.chars().all(is_ident) {
        return false;
    }
    if let Some(rhs) = find_go_type_rhs(source_text, type_expr) {
        return eval_rhs(source_text, rhs, source_file);
    }
    let Some(dir) = source_file.and_then(Path::parent) else {
        return false;
    };
    go_files_in_dir(dir, false)
        .filter(|p| Some(p.as_path()) != source_file)
        .any(|p| {
            std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| {
                    find_go_type_rhs(&t, type_expr).map(|rhs| eval_rhs(&t, rhs, Some(&p)))
                })
                .unwrap_or(false)
        })
}

pub(crate) fn is_named_map_or_slice_type(
    source_text: &str,
    lit_type: &str,
    source_file: Option<&Path>,
    root: Option<&Path>,
) -> bool {
    let base_type = lit_type.split('[').next().unwrap_or(lit_type).trim();
    resolve_type_is_map_or_slice(source_text, base_type, source_file, root, 0)
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
    source_file: &Path,
    target_file: &Path,
    root: &Path,
) -> Result<(String, String)> {
    let target_dir = target_file.parent().unwrap_or(root);
    let target_import_path = go_module_import_path(target_dir)?;
    anyhow::ensure!(
        !source_text.contains(&format!("\"{target_import_path}\"")),
        "the target Go module is already imported and its local alias cannot be safely resolved"
    );
    let (replaced, calls) = rewrite_go_call_sites_in_context(
        source_text,
        decl_name,
        target_pkg,
        Some(source_file),
        Some(root),
    )?;
    if calls == 0 {
        return Ok((source_text.to_string(), String::new()));
    }
    let with_import = add_go_import(&replaced, &target_import_path);
    Ok((with_import, format!("imported \"{target_import_path}\"")))
}
