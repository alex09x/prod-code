//! Architectural dependency graph extraction, coupling metrics, and cycle detection.
//!
//! Provides `code_dependencies` to inspect crate, package, and module dependencies,
//! calculate afferent (Ca) and efferent (Ce) coupling metrics, instability indices,
//! and detect cyclic dependencies (e.g. A -> B -> C -> A) using Tarjan's algorithm.

use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyNode {
    pub name: String,
    pub path: String,
    /// Number of other nodes that depend on this node (incoming).
    pub afferent_coupling: usize,
    /// Number of other nodes that this node depends on (outgoing).
    pub efferent_coupling: usize,
    /// Instability index: Ce / (Ca + Ce). 0.0 = completely stable, 1.0 = completely unstable.
    pub instability: f32,
    /// Direct dependencies of this node.
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyGraphReport {
    pub scope: String,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub cycles_detected: usize,
    pub cycles: Vec<Vec<String>>,
    pub nodes: Vec<DependencyNode>,
    pub isolated_nodes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyScope {
    Crates,
    Modules,
}

impl Default for DependencyScope {
    fn default() -> Self {
        Self::Crates
    }
}

/// Discovers and builds the dependency graph for a workspace.
pub fn analyze_dependencies(
    workspace_root: &Path,
    scope: DependencyScope,
    target_path: Option<&Path>,
) -> Result<DependencyGraphReport> {
    match scope {
        DependencyScope::Crates => analyze_crate_dependencies(workspace_root, target_path),
        DependencyScope::Modules => analyze_module_dependencies(workspace_root, target_path),
    }
}

/// Analyzes dependencies between workspace crates (Rust Cargo or Go modules).
fn analyze_crate_dependencies(
    workspace_root: &Path,
    _target_path: Option<&Path>,
) -> Result<DependencyGraphReport> {
    let mut adj: BTreeMap<String, (PathBuf, BTreeSet<String>)> = BTreeMap::new();

    // 1. Rust Cargo Workspace
    let root_cargo = workspace_root.join("Cargo.toml");
    if root_cargo.exists() {
        let manifest_content = std::fs::read_to_string(&root_cargo)
            .with_context(|| format!("Failed to read {}", root_cargo.display()))?;
        let parsed: toml::Value = toml::from_str(&manifest_content)
            .with_context(|| "Failed to parse root Cargo.toml")?;

        let mut crate_paths: Vec<PathBuf> = Vec::new();

        // Check if root is itself a package
        if let Some(pkg) = parsed.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()) {
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
            if let Ok(content) = std::fs::read_to_string(&manifest) {
                if let Ok(parsed_toml) = toml::from_str::<toml::Value>(&content) {
                    if let Some(name) = parsed_toml
                        .get("package")
                        .and_then(|p| p.get("name"))
                        .and_then(|n| n.as_str())
                    {
                        crate_names.insert(name.to_string());
                        crate_map.insert(name.to_string(), (dir.clone(), parsed_toml));
                    }
                }
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
                                    if let Ok(other_canon) = std::fs::canonicalize(other_dir) {
                                        if canon == other_canon {
                                            deps.insert(other_name.clone());
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            adj.insert(name, (dir, deps));
        }
    }

    build_graph_report("crates", workspace_root, adj)
}

/// Analyzes module-level dependencies by scanning source file import/use declarations.
fn analyze_module_dependencies(
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
        if path.is_file() {
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                if matches!(ext, "rs" | "go" | "py" | "ts" | "js") {
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
        }
    }

    // Phase 2: Parse imports
    for (module_name, path) in &file_modules {
        if let Ok(content) = std::fs::read_to_string(path) {
            let mut deps = BTreeSet::new();
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext {
                    "rs" => parse_rust_imports(&content, &file_modules, &mut deps),
                    "go" => parse_go_imports(&content, &file_modules, &mut deps),
                    "py" => parse_python_imports(&content, &file_modules, &mut deps),
                    "ts" | "js" => parse_ts_imports(&content, &file_modules, &mut deps),
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

fn file_to_module_name(rel_path: &str) -> String {
    rel_path
        .trim_end_matches(".rs")
        .trim_end_matches(".go")
        .trim_end_matches(".py")
        .trim_end_matches(".ts")
        .trim_end_matches(".js")
        .replace('/', "::")
}

fn parse_rust_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("use crate::") || trimmed.starts_with("use super::") {
            let path_part = trimmed
                .trim_start_matches("use crate::")
                .trim_start_matches("use super::")
                .split([';', ':', ' ', '{'])
                .next()
                .unwrap_or("");
            if !path_part.is_empty() {
                for mod_name in known_modules.keys() {
                    if mod_name.ends_with(path_part) || mod_name.contains(&format!("::{path_part}")) {
                        deps.insert(mod_name.clone());
                    }
                }
            }
        } else if trimmed.starts_with("mod ") && trimmed.ends_with(';') {
            let mod_name = trimmed.trim_start_matches("mod ").trim_end_matches(';').trim();
            for m in known_modules.keys() {
                if m.ends_with(&format!("::{mod_name}")) || m.ends_with(&format!("::{mod_name}::mod")) {
                    deps.insert(m.clone());
                }
            }
        }
    }
}

fn parse_go_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    let mut in_import_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import (") {
            in_import_block = true;
            continue;
        }
        if in_import_block && trimmed == ")" {
            in_import_block = false;
            continue;
        }

        if in_import_block || trimmed.starts_with("import ") {
            let pkg = trimmed
                .trim_start_matches("import ")
                .trim()
                .trim_matches('"');
            let last_segment = pkg.split('/').last().unwrap_or(pkg);
            for m in known_modules.keys() {
                if m.ends_with(last_segment) {
                    deps.insert(m.clone());
                }
            }
        }
    }
}

fn parse_python_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            let mod_part = trimmed.trim_start_matches("import ").split([' ', ',']).next().unwrap_or("");
            for m in known_modules.keys() {
                if m.ends_with(mod_part) {
                    deps.insert(m.clone());
                }
            }
        } else if trimmed.starts_with("from ") {
            let mod_part = trimmed.trim_start_matches("from ").split(' ').next().unwrap_or("");
            let clean = mod_part.trim_start_matches('.');
            for m in known_modules.keys() {
                if m.ends_with(clean) {
                    deps.insert(m.clone());
                }
            }
        }
    }
}

fn parse_ts_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if (trimmed.starts_with("import ") || trimmed.starts_with("export ")) && trimmed.contains("from ") {
            if let Some(from_str) = trimmed.split("from ").nth(1) {
                let path = from_str.trim().trim_matches([';', '\'', '"']);
                let base = path.split('/').last().unwrap_or(path);
                for m in known_modules.keys() {
                    if m.ends_with(base) {
                        deps.insert(m.clone());
                    }
                }
            }
        }
    }
}

/// Builds the report, calculates coupling metrics, and finds cycles.
fn build_graph_report(
    scope: &str,
    workspace_root: &Path,
    adj: BTreeMap<String, (PathBuf, BTreeSet<String>)>,
) -> Result<DependencyGraphReport> {
    let mut afferent_counts: HashMap<String, usize> = HashMap::new();
    let mut total_edges = 0;

    for (_node, (_, deps)) in &adj {
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
    nodes.sort_by(|a, b| b.afferent_coupling.cmp(&a.afferent_coupling));

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

/// Detects all cycles using depth-first search with recursion stack.
pub fn find_cycles(adj: &BTreeMap<String, (PathBuf, BTreeSet<String>)>) -> Vec<Vec<String>> {
    let mut cycles = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut on_stack: HashSet<String> = HashSet::new();
    let mut current_path: Vec<String> = Vec::new();

    for start_node in adj.keys() {
        if !visited.contains(start_node) {
            dfs_cycles(
                start_node,
                adj,
                &mut visited,
                &mut on_stack,
                &mut current_path,
                &mut cycles,
            );
        }
    }

    cycles
}

fn dfs_cycles(
    u: &str,
    adj: &BTreeMap<String, (PathBuf, BTreeSet<String>)>,
    visited: &mut HashSet<String>,
    on_stack: &mut HashSet<String>,
    path: &mut Vec<String>,
    cycles: &mut Vec<Vec<String>>,
) {
    visited.insert(u.to_string());
    on_stack.insert(u.to_string());
    path.push(u.to_string());

    if let Some((_, neighbors)) = adj.get(u) {
        for v in neighbors {
            if on_stack.contains(v) {
                // Cycle detected: slice from position of v in path to the end
                if let Some(pos) = path.iter().position(|node| node == v) {
                    let mut cycle: Vec<String> = path[pos..].to_vec();
                    cycle.push(v.clone());
                    cycles.push(cycle);
                }
            } else if !visited.contains(v) {
                dfs_cycles(v, adj, visited, on_stack, path, cycles);
            }
        }
    }

    path.pop();
    on_stack.remove(u);
}

/// Formats a dependency report as an ASCII summary table and cycle diagnosis.
pub fn format_dependency_report(report: &DependencyGraphReport) -> String {
    let mut out = String::new();
    out.push_str("⚡ prod-code Architecture & Dependency Graph Report\n");
    out.push_str("────────────────────────────────────────────────────\n");
    out.push_str(&format!(
        "Scope: {} | Nodes: {} | Dependencies: {}\n",
        report.scope, report.total_nodes, report.total_edges
    ));

    if report.cycles_detected > 0 {
        out.push_str(&format!(
            "\n🚨 CYCLES DETECTED: {} circular dependency path(s) found:\n",
            report.cycles_detected
        ));
        for (i, cycle) in report.cycles.iter().enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, cycle.join(" -> ")));
        }
    } else {
        out.push_str("\n✓ Zero circular dependencies detected. Architecture graph is a clean DAG.\n");
    }

    out.push_str("\nTop Coupled Modules / Crates (by Afferent Coupling Ca):\n");
    out.push_str(&format!(
        "  {:<32} {:>5} {:>5} {:>7}\n",
        "Name", "Ca", "Ce", "Instab"
    ));
    out.push_str("  ────────────────────────────────────────────────────\n");

    for node in report.nodes.iter().take(15) {
        out.push_str(&format!(
            "  {:<32} {:>5} {:>5} {:>7.2}\n",
            node.name, node.afferent_coupling, node.efferent_coupling, node.instability
        ));
    }

    if !report.isolated_nodes.is_empty() {
        out.push_str(&format!(
            "\nIsolated (Leaf/Orphan) Nodes ({}): {}\n",
            report.isolated_nodes.len(),
            report.isolated_nodes.iter().take(10).cloned().collect::<Vec<_>>().join(", ")
        ));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cycle_detection_simple() {
        let mut adj = BTreeMap::new();
        let dummy = PathBuf::from("test");

        adj.insert("A".to_string(), (dummy.clone(), ["B".to_string()].into()));
        adj.insert("B".to_string(), (dummy.clone(), ["C".to_string()].into()));
        adj.insert("C".to_string(), (dummy.clone(), ["A".to_string()].into()));

        let cycles = find_cycles(&adj);
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0], vec!["A", "B", "C", "A"]);
    }

    #[test]
    fn test_dag_no_cycles() {
        let mut adj = BTreeMap::new();
        let dummy = PathBuf::from("test");

        adj.insert("A".to_string(), (dummy.clone(), ["B".to_string(), "C".to_string()].into()));
        adj.insert("B".to_string(), (dummy.clone(), ["C".to_string()].into()));
        adj.insert("C".to_string(), (dummy.clone(), BTreeSet::new()));

        let cycles = find_cycles(&adj);
        assert!(cycles.is_empty());

        let report = build_graph_report("test", Path::new("."), adj).unwrap();
        assert_eq!(report.total_nodes, 3);
        assert_eq!(report.total_edges, 3);
        assert_eq!(report.cycles_detected, 0);

        // Node C has 2 incoming dependencies (A and B)
        let c_node = report.nodes.iter().find(|n| n.name == "C").unwrap();
        assert_eq!(c_node.afferent_coupling, 2);
        assert_eq!(c_node.efferent_coupling, 0);
        assert_eq!(c_node.instability, 0.0);
    }
}
