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

/// Whether a missing path stays inside the checkout without traversing symlinks.
pub(crate) fn missing_path_is_confined_to(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return false;
    };
    if relative
        .components()
        .any(|component| component == std::path::Component::ParentDir)
    {
        return false;
    }

    let mut ancestor = path.parent();
    while let Some(candidate) = ancestor {
        match std::fs::symlink_metadata(candidate) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return false;
                }
                let Ok(resolved) = std::fs::canonicalize(candidate) else {
                    return false;
                };
                if !resolved.starts_with(root) {
                    return false;
                }
                if candidate == root {
                    return true;
                }
                ancestor = candidate.parent();
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = candidate.parent();
            }
            Err(_) => return false,
        }
    }
    false
}

/// Whether a path is inside a dependency or build artifact directory.
pub fn is_in_dependency_dir(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        matches!(
            s.as_ref(),
            "target"
                | "node_modules"
                | "vendor"
                | "build"
                | "dist"
                | ".build"
                | ".venv"
                | "venv"
                | "Pods"
                | "DerivedData"
        )
    })
}

/// Is `dir` a Cargo crate that the workspace at `root` does not own?
///
/// Reads the root manifest's `[workspace]` table: a directory listed under `exclude` (by prefix)
/// or absent from a `members` list that has no glob covering it is not part of the workspace's
/// project model, however much it looks like one from the outside.
pub(crate) fn excluded_from_root_workspace(root: &Path, dir: &Path) -> bool {
    if !dir.join("Cargo.toml").is_file() {
        return false;
    }
    let Ok(manifest) = std::fs::read_to_string(root.join("Cargo.toml")) else {
        return false;
    };
    let Ok(rel) = dir.strip_prefix(root) else {
        return false;
    };
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    if rel.is_empty() {
        return false;
    }
    let (members, excludes) = workspace_lists(&manifest);
    if excludes
        .iter()
        .any(|e| rel == *e || rel.starts_with(&format!("{e}/")))
    {
        return true;
    }
    // No `members` at all: the manifest is a plain package, and a crate below it is its own.
    if members.is_empty() {
        return !manifest.contains("[workspace]");
    }
    !members.iter().any(|m| match m.strip_suffix("/*") {
        Some(prefix) => {
            rel.starts_with(&format!("{prefix}/")) && rel[prefix.len() + 1..].find('/').is_none()
        }
        None => rel == *m,
    })
}

/// The `members` and `exclude` entries of a root manifest's `[workspace]` table, read without a
/// TOML parser: both are arrays of plain strings, and this only has to recognise them.
pub(crate) fn workspace_lists(manifest: &str) -> (Vec<String>, Vec<String>) {
    let mut members = Vec::new();
    let mut excludes = Vec::new();
    let mut target: Option<&mut Vec<String>> = None;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            target = None;
        }
        let start = |key: &str| {
            trimmed
                .strip_prefix(key)
                .map(|rest| rest.trim_start().starts_with('='))
                .unwrap_or(false)
        };
        if start("members") {
            members.extend(entries_on(trimmed));
            target = if trimmed.trim_end().ends_with(']') {
                None
            } else {
                Some(&mut members)
            };
            continue;
        }
        if start("exclude") {
            excludes.extend(entries_on(trimmed));
            target = if trimmed.trim_end().ends_with(']') {
                None
            } else {
                Some(&mut excludes)
            };
            continue;
        }
        if let Some(list) = target.as_deref_mut() {
            list.extend(entries_on(trimmed));
            if trimmed.starts_with(']') || trimmed.ends_with(']') {
                target = None;
            }
        }
    }
    (members, excludes)
}

/// The quoted strings on one line of a TOML array.
pub(crate) fn entries_on(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let value = after[..close].trim_matches('/').to_string();
        if !value.is_empty() {
            out.push(value);
        }
        rest = &after[close + 1..];
    }
    out
}

pub(crate) fn has_c_sources(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| {
                    matches!(x, "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx")
                })
        })
    })
}

pub(crate) fn mcp_directory_has_kotlin_source(dir: &Path, depth: usize) -> bool {
    if depth > 6 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.')
            || matches!(
                name.as_str(),
                "build" | "target" | "node_modules" | ".gradle"
            )
        {
            return false;
        }
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            mcp_directory_has_kotlin_source(&path, depth + 1)
        } else {
            path.extension().and_then(|extension| extension.to_str()) == Some("kt")
        }
    })
}

pub(crate) fn mcp_has_kotlin_project(root: &Path) -> bool {
    if ["src", "app/src", "common/src", "shared/src"]
        .iter()
        .any(|relative| mcp_directory_has_kotlin_source(&root.join(relative), 0))
    {
        return true;
    }
    [
        "build.gradle",
        "build.gradle.kts",
        "settings.gradle",
        "settings.gradle.kts",
    ]
    .iter()
    .filter_map(|name| std::fs::read_to_string(root.join(name)).ok())
    .any(|text| {
        [
            "org.jetbrains.kotlin",
            "kotlin(\"jvm\")",
            "kotlin(\"android\")",
            "kotlin(\"multiplatform\")",
        ]
        .iter()
        .any(|marker| text.contains(marker))
    })
}
