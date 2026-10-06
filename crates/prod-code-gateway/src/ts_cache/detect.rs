/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::path::{Path, PathBuf};

use super::roots::{
    VisitedDirs, approved_target, build_approved_roots, find_enclosing_project_root,
    is_target_approved,
};

/// Checks whether `root` represents or contains a TypeScript or JavaScript project.
pub fn is_typescript_project(root: &Path) -> bool {
    const TS_MARKERS: &[&str] = &[
        "tsconfig.json",
        "jsconfig.json",
        "package.json",
        "deno.json",
        "deno.jsonc",
        "bunfig.toml",
    ];

    for marker in TS_MARKERS {
        if root.join(marker).is_file() {
            return true;
        }
    }

    // Check top-level or immediate subfolder source files
    let subdirs = ["src", "lib", "test", "tests", "packages", "apps", "."];
    for sub in &subdirs {
        let check_dir = if *sub == "." {
            root.to_path_buf()
        } else {
            root.join(sub)
        };
        if let Ok(entries) = fs::read_dir(&check_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if matches!(
                        ext,
                        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts"
                    ) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Discovers candidate type declaration roots in a project.
///
/// Discovers:
/// 1. `root/node_modules/@types`
/// 2. Monorepo subpackage `node_modules/@types` (e.g. `packages/*/node_modules/@types`)
/// 3. Project-level custom `types/`, `@types/`, `typings/` containing declaration files.
/// Constrains candidate discovery to approved roots and avoids symlink cycles (#836).
pub fn find_project_types(root: &Path) -> Vec<PathBuf> {
    let project_root = find_enclosing_project_root(root);
    find_project_types_within(root, &[&project_root])
}

/// Discovers candidate type declaration roots in a project constraining candidate paths
/// and recursive declaration file checks to explicit approved roots (#836).
pub fn find_project_types_within(root: &Path, approved_roots: &[&Path]) -> Vec<PathBuf> {
    let approved = build_approved_roots(approved_roots);
    let mut type_dirs = Vec::new();

    // 1. Root node_modules/@types
    let root_types = root.join("node_modules").join("@types");
    if root_types.is_dir() && is_target_approved(&root_types, &approved) {
        type_dirs.push(root_types);
    }

    // 2. Custom local types directories
    for custom_name in &["types", "@types", "typings"] {
        let custom_dir = root.join(custom_name);
        if custom_dir.is_dir()
            && is_target_approved(&custom_dir, &approved)
            && has_declaration_files_inner(&custom_dir, &approved, &mut VisitedDirs::default())
        {
            type_dirs.push(custom_dir);
        }
    }

    // 3. Monorepo subpackages: packages/*, apps/*, libs/*
    for mono_parent in &["packages", "apps", "libs", "modules"] {
        let parent_dir = root.join(mono_parent);
        if let Ok(entries) = fs::read_dir(&parent_dir) {
            for entry in entries.flatten() {
                let pkg_dir = entry.path();
                if pkg_dir.is_dir() && is_target_approved(&pkg_dir, &approved) {
                    let sub_at_types = pkg_dir.join("node_modules").join("@types");
                    if sub_at_types.is_dir() && is_target_approved(&sub_at_types, &approved) {
                        type_dirs.push(sub_at_types);
                    }
                    for custom_name in &["types", "@types", "typings"] {
                        let sub_custom = pkg_dir.join(custom_name);
                        if sub_custom.is_dir()
                            && is_target_approved(&sub_custom, &approved)
                            && has_declaration_files_inner(
                                &sub_custom,
                                &approved,
                                &mut VisitedDirs::default(),
                            )
                        {
                            type_dirs.push(sub_custom);
                        }
                    }
                }
            }
        }
    }

    type_dirs
}

/// Checks if a directory contains any `.d.ts`, `.d.mts`, or `.d.cts` files,
/// safely dereferencing symlinks only within approved roots and tracking visited directory inodes to prevent cycles (#836).
pub fn has_declaration_files(dir: &Path) -> bool {
    let project_root = find_enclosing_project_root(dir);
    has_declaration_files_within(dir, &[&project_root])
}

/// Checks if a directory contains declaration files, constraining symlinks to explicit approved roots.
pub fn has_declaration_files_within(dir: &Path, approved_roots: &[&Path]) -> bool {
    let approved = build_approved_roots(approved_roots);
    let mut visited = VisitedDirs::default();
    has_declaration_files_inner(dir, &approved, &mut visited)
}

pub(crate) fn has_declaration_files_inner(
    dir: &Path,
    approved_roots: &[PathBuf],
    visited: &mut VisitedDirs,
) -> bool {
    let Some(canonical_dir) = approved_target(dir, approved_roots) else {
        return false;
    };
    if !canonical_dir.is_dir() || !visited.insert(&canonical_dir) {
        return false;
    }
    let Ok(entries) = fs::read_dir(&canonical_dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }

        // Safely dereference symlinks within approved roots, using the validated canonical target
        // for subsequent operations to prevent TOCTOU symlink swaps (#836).
        let (effective_path, is_dir, is_file) = if file_type.is_symlink() {
            let Some(canon) = approved_target(&path, approved_roots) else {
                continue;
            };
            match fs::metadata(&canon) {
                Ok(meta) => (canon, meta.is_dir(), meta.is_file()),
                Err(_) => continue,
            }
        } else {
            (path, file_type.is_dir(), file_type.is_file())
        };

        if is_file {
            if name_str.ends_with(".d.ts")
                || name_str.ends_with(".d.mts")
                || name_str.ends_with(".d.cts")
            {
                return true;
            }
        } else if is_dir && has_declaration_files_inner(&effective_path, approved_roots, visited) {
            return true;
        }
    }
    false
}
