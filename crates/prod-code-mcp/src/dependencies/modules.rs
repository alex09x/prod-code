/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use ignore::WalkBuilder;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::cycles::find_cycles;
use super::languages::{
    parse_csharp_imports, parse_go_imports, parse_java_imports, parse_python_imports,
    parse_rust_imports, parse_ts_imports,
};
use super::types::{DependencyGraphReport, DependencyNode};

pub(crate) fn file_to_module_name(rel_path: &str) -> String {
    let p = if let Some(idx) = rel_path.rfind('.') {
        &rel_path[..idx]
    } else {
        rel_path
    };
    p.replace('/', "::")
}

/// Analyzes module-level dependencies by scanning source file import/use declarations.
pub fn analyze_module_dependencies(
    workspace_root: &Path,
    target_path: Option<&Path>,
) -> Result<DependencyGraphReport> {
    let mut adj: BTreeMap<String, (PathBuf, BTreeSet<String>)> = BTreeMap::new();
    let scan_root = target_path.unwrap_or(workspace_root);

    let walker = WalkBuilder::new(scan_root)
        .hidden(true)
        .git_ignore(true)
        .build();

    let mut file_modules = HashMap::new();

    // Phase 1: Index files
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file()
            && let Some(ext) = path.extension().and_then(|e| e.to_str())
            && matches!(ext, "rs" | "go" | "py" | "ts" | "js" | "java" | "kt" | "cs")
        {
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();
            let module_name = file_to_module_name(&rel);
            file_modules.insert(module_name.clone(), path.to_path_buf());
            adj.entry(module_name)
                .or_insert_with(|| (path.to_path_buf(), BTreeSet::new()));
        }
    }

    if file_modules.is_empty() {
        return Err(anyhow::anyhow!(
            "dependency graph analysis found no supported source modules; supported languages are Rust, Go, Python, TypeScript/JavaScript, Java/Kotlin, and C#"
        ));
    }

    let go_module_name: Option<String> = std::fs::read_to_string(workspace_root.join("go.mod"))
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with("module ") {
                    Some(trimmed.trim_start_matches("module ").trim().to_string())
                } else {
                    None
                }
            })
        });

    // Phase 2: Parse imports
    for (module_name, path) in &file_modules {
        if let Ok(content) = std::fs::read_to_string(path) {
            let mut deps = BTreeSet::new();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext {
                    "rs" => parse_rust_imports(&content, &file_modules, &mut deps),
                    "go" => parse_go_imports(
                        &content,
                        go_module_name.as_deref(),
                        &file_modules,
                        &mut deps,
                    ),
                    "py" => parse_python_imports(&content, &file_modules, &mut deps),
                    "ts" | "js" => parse_ts_imports(&content, &file_modules, &mut deps),
                    "java" | "kt" => parse_java_imports(&content, &file_modules, &mut deps),
                    "cs" => parse_csharp_imports(&content, &file_modules, &mut deps),
                    _ => {}
                }
            }
            // Remove self-dependency
            deps.remove(module_name);
            if let Some(entry) = adj.get_mut(module_name) {
                entry.1 = deps;
            }
        }
    }

    build_graph_report("modules", workspace_root, adj)
}

/// Builds the report, calculates coupling metrics, and finds cycles.
pub fn build_graph_report(
    scope: &str,
    workspace_root: &Path,
    adj: BTreeMap<String, (PathBuf, BTreeSet<String>)>,
) -> Result<DependencyGraphReport> {
    let mut afferent_counts: HashMap<String, usize> = HashMap::new();
    let mut total_edges = 0;

    for (_, deps) in adj.values() {
        total_edges += deps.len();
        for dep in deps {
            *afferent_counts.entry(dep.clone()).or_insert(0) += 1;
        }
    }

    let cycles = find_cycles(&adj);

    let mut nodes = Vec::new();
    let mut isolated_nodes = Vec::new();

    for (name, (path, deps)) in &adj {
        let ca = *afferent_counts.get(name).unwrap_or(&0);
        let ce = deps.len();
        let instability = if ca + ce == 0 {
            0.0
        } else {
            ce as f32 / (ca + ce) as f32
        };

        if ca == 0 && ce == 0 {
            isolated_nodes.push(name.clone());
        }

        let rel_path = path
            .strip_prefix(workspace_root)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        nodes.push(DependencyNode {
            name: name.clone(),
            path: rel_path,
            afferent_coupling: ca,
            efferent_coupling: ce,
            instability,
            dependencies: deps.iter().cloned().collect(),
        });
    }

    // Sort nodes by coupling (most depended-on first)
    nodes.sort_by_key(|n| std::cmp::Reverse(n.afferent_coupling));

    Ok(DependencyGraphReport {
        scope: scope.to_string(),
        total_nodes: adj.len(),
        total_edges,
        cycles_detected: cycles.len(),
        cycles,
        nodes,
        isolated_nodes,
    })
}
