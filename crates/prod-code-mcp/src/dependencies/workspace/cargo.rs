/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

pub fn collect_cargo_dependencies(
    workspace_root: &Path,
    adj: &mut BTreeMap<String, (PathBuf, BTreeSet<String>)>,
) -> Result<()> {
    let root_cargo = workspace_root.join("Cargo.toml");
    if !root_cargo.exists() {
        return Ok(());
    }

    let manifest_content = std::fs::read_to_string(&root_cargo)
        .with_context(|| format!("Failed to read {}", root_cargo.display()))?;
    let parsed: toml::Value =
        toml::from_str(&manifest_content).with_context(|| "Failed to parse root Cargo.toml")?;

    let mut crate_paths: Vec<PathBuf> = Vec::new();

    // Check if root is itself a package
    if let Some(pkg) = parsed
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
    {
        adj.entry(pkg.to_string())
            .or_insert_with(|| (workspace_root.to_path_buf(), BTreeSet::new()));
    }

    // Check workspace members
    if let Some(members) = parsed
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
    {
        for m in members {
            if let Some(pattern) = m.as_str() {
                if pattern.contains('*') {
                    // Handle simple glob e.g. "crates/*"
                    let prefix = pattern.trim_end_matches('*').trim_end_matches('/');
                    let search_dir = workspace_root.join(prefix);
                    if let Ok(entries) = std::fs::read_dir(&search_dir) {
                        for entry in entries.flatten() {
                            let cargo = entry.path().join("Cargo.toml");
                            if cargo.exists() {
                                crate_paths.push(entry.path());
                            }
                        }
                    }
                } else {
                    let dir = workspace_root.join(pattern);
                    if dir.join("Cargo.toml").exists() {
                        crate_paths.push(dir);
                    }
                }
            }
        }
    }

    // Collect all crate names first
    let mut crate_names = HashSet::new();
    let mut crate_map = HashMap::new();
    for dir in &crate_paths {
        let manifest = dir.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&manifest)
            && let Ok(parsed_toml) = toml::from_str::<toml::Value>(&content)
            && let Some(name) = parsed_toml
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
        {
            crate_names.insert(name.to_string());
            crate_map.insert(name.to_string(), (dir.clone(), parsed_toml));
        }
    }

    for (name, (dir, toml_val)) in crate_map {
        let mut deps = BTreeSet::new();
        for dep_table in ["dependencies", "dev-dependencies", "build-dependencies"] {
            if let Some(tbl) = toml_val.get(dep_table).and_then(|t| t.as_table()) {
                for (dep_name, dep_val) in tbl {
                    if crate_names.contains(dep_name) {
                        deps.insert(dep_name.clone());
                    } else if let Some(path_dep) = dep_val.get("path").and_then(|p| p.as_str()) {
                        // Path-based dependency
                        let resolved_path = dir.join(path_dep);
                        if let Ok(canon) = std::fs::canonicalize(&resolved_path) {
                            for (other_name, (other_dir, _)) in adj.iter() {
                                if let Ok(other_canon) = std::fs::canonicalize(other_dir)
                                    && canon == other_canon
                                {
                                    deps.insert(other_name.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        adj.insert(name, (dir, deps));
    }

    Ok(())
}
