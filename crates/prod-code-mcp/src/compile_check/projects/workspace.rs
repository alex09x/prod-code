/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{ManifestOverlay, join_checked, slash_path};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use toml::Value;

pub(super) fn nearest_manifest_dir(overlay: &ManifestOverlay<'_>, start: &Path) -> Option<PathBuf> {
    let mut cur = start.to_path_buf();
    loop {
        if overlay.read_manifest(&cur).is_some() {
            return Some(cur);
        }
        if cur.as_os_str().is_empty() {
            return None;
        }
        cur = cur.parent().unwrap_or(Path::new("")).to_path_buf();
    }
}

pub(super) fn resolve_package_group_dir(overlay: &ManifestOverlay<'_>, pkg: &Path) -> PathBuf {
    let Some(doc) = overlay.parse_manifest(pkg) else {
        return pkg.to_path_buf();
    };
    if doc.get("workspace").and_then(Value::as_table).is_some() {
        return pkg.to_path_buf();
    }
    if let Some(pointer) = doc
        .get("package")
        .and_then(|v| v.get("workspace"))
        .and_then(Value::as_str)
        && let Some(ws) = join_checked(pkg, Path::new(pointer))
        && workspace_owns_package(overlay, &ws, pkg)
    {
        return ws;
    }
    let mut cur = pkg.to_path_buf();
    while !cur.as_os_str().is_empty() {
        cur = cur.parent().unwrap_or(Path::new("")).to_path_buf();
        if workspace_owns_package(overlay, &cur, pkg) {
            return cur;
        }
    }
    pkg.to_path_buf()
}

fn workspace_owns_package(overlay: &ManifestOverlay<'_>, ws: &Path, pkg: &Path) -> bool {
    let Ok(rel) = pkg.strip_prefix(ws) else {
        return false;
    };
    let Some(doc) = overlay.parse_manifest(ws) else {
        return false;
    };
    let Some(table) = doc.get("workspace").and_then(Value::as_table) else {
        return false;
    };
    if rel.as_os_str().is_empty() {
        return true;
    }
    let excludes = string_list(table.get("exclude"));
    if excluded(rel, &excludes) {
        return false;
    }
    let members = string_list(table.get("members"));
    if member_matches(rel, &members) {
        return true;
    }
    reachable(overlay, ws, &doc, &members, &excludes, pkg)
}

fn excluded(rel: &Path, excludes: &[String]) -> bool {
    excludes.iter().any(|raw| {
        let pat = clean_pattern(raw);
        if pat.is_empty() {
            return false;
        }
        if rel == Path::new(pat) || rel.starts_with(pat) {
            return true;
        }
        let Ok(glob) = glob::Pattern::new(pat) else {
            return false;
        };
        let mut cur = Some(rel);
        while let Some(prefix) = cur {
            if !prefix.as_os_str().is_empty() && glob.matches(&slash_path(prefix)) {
                return true;
            }
            cur = prefix.parent();
        }
        false
    })
}

fn member_matches(rel: &Path, members: &[String]) -> bool {
    members.iter().any(|raw| {
        let pat = clean_pattern(raw);
        !pat.is_empty()
            && (rel == Path::new(pat)
                || glob::Pattern::new(pat).is_ok_and(|g| g.matches(&slash_path(rel))))
    })
}

fn reachable(
    overlay: &ManifestOverlay<'_>,
    ws: &Path,
    doc: &Value,
    members: &[String],
    excludes: &[String],
    target: &Path,
) -> bool {
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    // A virtual workspace's definitions are not edges. Only actual packages seed the walk.
    if doc.get("package").is_some() {
        queue.push_back(ws.to_path_buf());
    }
    for file in &overlay.files {
        if file.file_name().is_some_and(|n| n == "Cargo.toml")
            && let Some(dir) = file.parent()
            && let Ok(rel) = dir.strip_prefix(ws)
            && member_matches(rel, members)
            && !excluded(rel, excludes)
        {
            queue.push_back(dir.to_path_buf());
        }
    }
    while let Some(dir) = queue.pop_front() {
        if !visited.insert(dir.clone()) {
            continue;
        }
        if dir == target {
            return true;
        }
        let Some(package) = overlay.parse_manifest(&dir) else {
            continue;
        };
        for dependency in dependency_paths(&package, &dir, doc, ws) {
            if let Ok(rel) = dependency.strip_prefix(ws)
                && !excluded(rel, excludes)
            {
                queue.push_back(dependency);
            }
        }
    }
    false
}

/// Resolve inherited definitions at their use site, relative to the workspace;
/// ordinary paths remain relative to the package that declares the edge.
fn dependency_paths(package: &Value, dir: &Path, workspace: &Value, ws: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut collect = |section: Option<&Value>| {
        let Some(table) = section.and_then(Value::as_table) else {
            return;
        };
        for (name, spec) in table {
            let inherited = spec.get("workspace").and_then(Value::as_bool) == Some(true);
            let (definition, base) = if inherited {
                (
                    workspace
                        .get("workspace")
                        .and_then(|w| w.get("dependencies"))
                        .and_then(|d| d.get(name)),
                    ws,
                )
            } else {
                (Some(spec), dir)
            };
            if let Some(path) = definition
                .and_then(|d| d.get("path"))
                .and_then(Value::as_str)
                && let Some(path) = join_checked(base, Path::new(path))
            {
                out.push(path);
            }
        }
    };
    for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
        collect(package.get(section));
    }
    if let Some(targets) = package.get("target").and_then(Value::as_table) {
        for target in targets.values() {
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                collect(target.get(section));
            }
        }
    }
    out
}

fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn clean_pattern(raw: &str) -> &str {
    raw.trim().trim_start_matches("./").trim_end_matches('/')
}
