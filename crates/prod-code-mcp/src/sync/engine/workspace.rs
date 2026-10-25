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
/// Only `[workspace]` describes membership; `[package].exclude` controls packaging.
/// A child outside the members (including an empty virtual workspace) needs its own root.
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
    if rel.as_os_str().is_empty() {
        return false;
    }
    let (members, excludes) = workspace_lists(&manifest);
    if workspace_excludes(&excludes, rel) {
        return true;
    }
    if members
        .iter()
        .any(|pattern| cargo_pattern_matches(pattern, rel))
    {
        return false;
    }
    !implicit_path_member(root, dir, &manifest, &members, &excludes)
}

fn normalized_path(path: &Path) -> std::path::PathBuf {
    path.components()
        .filter(|component| !matches!(component, std::path::Component::CurDir))
        .collect()
}

fn workspace_excludes(excludes: &[String], relative: &Path) -> bool {
    // Cargo expands member globs, but exclusions are literal directory prefixes.
    // In particular, excluding `crates/foo-*` must not exclude `crates/foo-one`.
    let relative = normalized_path(relative);
    excludes
        .iter()
        .any(|exclude| relative.starts_with(normalized_path(Path::new(exclude))))
}

/// Cargo also includes in-tree path dependencies of workspace packages, recursively.
/// Discover only packages reachable from the root package or explicit members; a
/// crate merely sitting below an empty virtual workspace is still independent.
fn implicit_path_member(
    root: &Path,
    dir: &Path,
    manifest: &str,
    members: &[String],
    excludes: &[String],
) -> bool {
    let Ok(manifest) = toml::from_str::<toml::Value>(manifest) else {
        return false;
    };
    if manifest
        .get("workspace")
        .and_then(toml::Value::as_table)
        .is_none()
    {
        return false;
    }
    let mut pending = vec![root.to_path_buf()];
    for member in members {
        // Escape the checkout's spelling, retaining glob syntax only in the member.
        let pattern = Path::new(&glob::Pattern::escape(&root.to_string_lossy())).join(member);
        if let Some(pattern) = pattern.to_str()
            && let Ok(paths) = glob::glob(pattern)
        {
            pending.extend(paths.flatten());
        }
    }
    let mut seen = std::collections::HashSet::new();
    while let Some(package) = pending.pop() {
        let Ok(package) = package.canonicalize() else {
            continue;
        };
        let Ok(relative) = package.strip_prefix(root) else {
            continue;
        };
        if workspace_excludes(excludes, relative) || !seen.insert(package.clone()) {
            continue;
        }
        let Some(parsed) = std::fs::read_to_string(package.join("Cargo.toml"))
            .ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
        else {
            continue;
        };
        if parsed.get("package").is_none() {
            continue;
        }
        if package == dir {
            return true;
        }
        dependency_paths(root, &package, &parsed, &manifest, &mut pending);
        if let Some(targets) = parsed.get("target").and_then(toml::Value::as_table) {
            for target in targets.values() {
                dependency_paths(root, &package, target, &manifest, &mut pending);
            }
        }
    }
    false
}

fn dependency_paths(
    root: &Path,
    package: &Path,
    manifest: &toml::Value,
    workspace: &toml::Value,
    pending: &mut Vec<std::path::PathBuf>,
) {
    for key in ["dependencies", "build-dependencies", "dev-dependencies"] {
        if let Some(dependencies) = manifest.get(key).and_then(toml::Value::as_table) {
            for (name, dependency) in dependencies {
                let (base, dependency) =
                    if dependency.get("workspace").and_then(toml::Value::as_bool) == Some(true) {
                        let Some(inherited) = workspace
                            .get("workspace")
                            .and_then(|table| table.get("dependencies"))
                            .and_then(|table| table.get(name))
                        else {
                            continue;
                        };
                        (root, inherited)
                    } else {
                        (package, dependency)
                    };
                if let Some(path) = dependency.get("path").and_then(toml::Value::as_str) {
                    pending.push(base.join(path));
                }
            }
        }
    }
}

/// Read the actual workspace table, including TOML literal/escaped strings and dotted keys.
pub(crate) fn workspace_lists(manifest: &str) -> (Vec<String>, Vec<String>) {
    let Ok(manifest) = toml::from_str::<toml::Value>(manifest) else {
        return (Vec::new(), Vec::new());
    };
    let strings = |key| {
        manifest
            .get("workspace")
            .and_then(|table| table.get(key))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(toml::Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    (strings("members"), strings("exclude"))
}

fn cargo_pattern_matches(pattern: &str, path: &Path) -> bool {
    // Normalize path spelling, not glob syntax: retain absolute roots and platform
    // separator rules, while accepting Cargo's `./crates/foo/` spelling.
    let pattern = normalized_path(Path::new(pattern));
    let path: std::path::PathBuf = path.components().collect();
    pattern.to_str().is_some_and(|pattern| {
        glob::Pattern::new(pattern).is_ok_and(|pattern| {
            pattern.matches_path_with(
                &path,
                glob::MatchOptions {
                    require_literal_separator: true,
                    ..Default::default()
                },
            )
        })
    })
}

pub(crate) fn has_c_sources(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| {
                    matches!(
                        x.to_ascii_lowercase().as_str(),
                        "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx"
                    )
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
