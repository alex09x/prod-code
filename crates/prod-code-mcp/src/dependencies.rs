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

    // 2. Maven Multi-Module Workspace (Java)
    let root_pom = workspace_root.join("pom.xml");
    if root_pom.exists() {
        if let Ok(pom_content) = std::fs::read_to_string(&root_pom) {
            let mut modules = Vec::new();
            let mut in_modules = false;
            for line in pom_content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("<modules>") {
                    in_modules = true;
                } else if trimmed.starts_with("</modules>") {
                    in_modules = false;
                } else if in_modules && trimmed.starts_with("<module>") && trimmed.ends_with("</module>") {
                    let mod_name = trimmed
                        .trim_start_matches("<module>")
                        .trim_end_matches("</module>")
                        .trim();
                    modules.push(mod_name.to_string());
                }
            }

            let mut module_map = HashMap::new();
            let mut artifact_to_mod = HashMap::new();
            for m in &modules {
                let sub_dir = workspace_root.join(m);
                let sub_pom = sub_dir.join("pom.xml");
                if let Ok(content) = std::fs::read_to_string(&sub_pom) {
                    if let Some(art) = extract_maven_artifact_id(&content) {
                        artifact_to_mod.insert(art, m.clone());
                    }
                    artifact_to_mod.insert(m.clone(), m.clone());
                    module_map.insert(m.clone(), (sub_dir, content));
                }
            }

            for (mod_name, (dir, content)) in &module_map {
                let mut deps = BTreeSet::new();
                for (art, target_mod) in &artifact_to_mod {
                    if target_mod != mod_name && content.contains(&format!("<artifactId>{art}</artifactId>")) {
                        deps.insert(target_mod.clone());
                    }
                }
                adj.insert(mod_name.clone(), (dir.clone(), deps));
            }
        }
    }

    // 3. Gradle Multi-Project Workspace (Java / Kotlin / Android)
    let root_gradle = workspace_root.join("settings.gradle");
    let root_gradle_kts = workspace_root.join("settings.gradle.kts");
    let gradle_settings_path = if root_gradle.exists() {
        Some(root_gradle)
    } else if root_gradle_kts.exists() {
        Some(root_gradle_kts)
    } else {
        None
    };

    if let Some(settings_file) = gradle_settings_path {
        if let Ok(settings_content) = std::fs::read_to_string(&settings_file) {
            let mut projects = Vec::new();
            let mut in_include = false;
            for line in settings_content.lines() {
                let trimmed = line.trim();
                if trimmed.starts_with("include ") || trimmed.starts_with("include(") {
                    in_include = true;
                }
                if in_include {
                    let code_part = if let Some(idx) = trimmed.find("//") {
                        &trimmed[..idx]
                    } else {
                        trimmed
                    };
                    let mut at = 0;
                    while let Some(q_start) = code_part[at..].find(['"', '\'']) {
                        let quote_char = code_part[at + q_start..].chars().next().unwrap();
                        let rest = &code_part[at + q_start + 1..];
                        if let Some(q_end) = rest.find(quote_char) {
                            let proj_name = rest[..q_end].trim_matches(':');
                            if !proj_name.is_empty() && !projects.contains(&proj_name.to_string()) {
                                projects.push(proj_name.to_string());
                            }
                            at += q_start + 1 + q_end + 1;
                        } else {
                            break;
                        }
                    }
                    if !trimmed.ends_with(',') && !trimmed.starts_with("include") {
                        in_include = false;
                    } else if trimmed.ends_with(')') {
                        in_include = false;
                    }
                }

                // Handle JetBrains module("path/name") helper DSL
                if trimmed.starts_with("module(") || trimmed.starts_with("module ") {
                    let rest = if trimmed.starts_with("module(") {
                        trimmed.strip_prefix("module(").unwrap().trim()
                    } else {
                        trimmed.strip_prefix("module ").unwrap().trim()
                    };
                    if (rest.starts_with('"') && rest.len() >= 2) || (rest.starts_with('\'') && rest.len() >= 2) {
                        let quote = rest.chars().next().unwrap();
                        if let Some(end_idx) = rest[1..].find(quote) {
                            let path_str = &rest[1..1 + end_idx];
                            let mod_name = path_str.split('/').last().unwrap_or(path_str).trim_matches(':');
                            if !mod_name.is_empty() && !projects.contains(&mod_name.to_string()) {
                                projects.push(mod_name.to_string());
                            }
                        }
                    }
                }

                // Handle Kotlin unaryPlus DSL: +"foo-bar" or + "foo-bar"
                if trimmed.starts_with('+') {
                    let rest = trimmed.trim_start_matches('+').trim();
                    if (rest.starts_with('"') && rest.len() >= 2) || (rest.starts_with('\'') && rest.len() >= 2) {
                        let quote = rest.chars().next().unwrap();
                        if let Some(end_idx) = rest[1..].find(quote) {
                            let proj_name = &rest[1..1 + end_idx];
                            let clean = proj_name.trim_matches(':');
                            if !clean.is_empty() && !projects.contains(&clean.to_string()) {
                                projects.push(clean.to_string());
                            }
                        }
                    }
                }
            }

            // Index all subdirectories containing build.gradle or build.gradle.kts to map project names to paths
            let mut dir_by_name: HashMap<String, PathBuf> = HashMap::new();
            let mut walker = WalkBuilder::new(workspace_root);
            walker.hidden(true).git_ignore(true);
            for entry in walker.build().flatten() {
                let path = entry.path();
                if path.is_file() {
                    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if file_name == "build.gradle" || file_name == "build.gradle.kts" {
                        if let Some(parent) = path.parent() {
                            let rel_dir = parent.strip_prefix(workspace_root).unwrap_or(parent);
                            let rel_str = rel_dir.to_string_lossy().to_string();
                            if !rel_str.is_empty()
                                && !rel_str.starts_with("build-")
                                && !rel_str.starts_with(".gradle")
                                && !rel_str.starts_with("build/")
                                && !rel_str.starts_with("buildSrc")
                            {
                                if let Some(folder_name) = parent.file_name().and_then(|n| n.to_str()) {
                                    dir_by_name.insert(folder_name.to_string(), parent.to_path_buf());
                                    dir_by_name.insert(rel_str.replace('/', ":"), parent.to_path_buf());
                                    dir_by_name.insert(rel_str.clone(), parent.to_path_buf());
                                    if !projects.contains(&folder_name.to_string()) {
                                        projects.push(folder_name.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let root_build_content = std::fs::read_to_string(workspace_root.join("build.gradle"))
                .or_else(|_| std::fs::read_to_string(workspace_root.join("build.gradle.kts")))
                .ok();

            let mut project_map = HashMap::new();
            for p in &projects {
                let sub_rel = p.replace(':', "/");
                let sub_dir = if workspace_root.join(&sub_rel).exists() {
                    workspace_root.join(&sub_rel)
                } else if let Some(d) = dir_by_name.get(p) {
                    d.clone()
                } else if let Some(last) = p.split(':').last() {
                    if let Some(d) = dir_by_name.get(last) {
                        d.clone()
                    } else {
                        workspace_root.join(&sub_rel)
                    }
                } else {
                    workspace_root.join(&sub_rel)
                };

                let build_gradle = sub_dir.join("build.gradle");
                let build_gradle_kts = sub_dir.join("build.gradle.kts");
                let content = if let Ok(c) = std::fs::read_to_string(&build_gradle) {
                    c
                } else if let Ok(c) = std::fs::read_to_string(&build_gradle_kts) {
                    c
                } else if let Some(ref root_content) = root_build_content {
                    extract_gradle_project_block(root_content, p).unwrap_or_default()
                } else {
                    String::new()
                };

                project_map.insert(p.clone(), (sub_dir, content));
            }

            for (proj_name, (dir, content)) in &project_map {
                let mut deps = BTreeSet::new();
                for other in project_map.keys() {
                    if other != proj_name {
                        let ref1 = format!("project(\":{other}\")");
                        let ref2 = format!("project(':{other}')");
                        let ref3 = format!("project(\"{other}\")");
                        let ref4 = format!("project('{other}')");
                        let ref5 = format!(":{other}");
                        let camel1 = format!("projects.{}", kebab_to_camel(other));
                        let camel2 = format!(
                            "projects.{}",
                            other.split(':').map(kebab_to_camel).collect::<Vec<_>>().join(".")
                        );
                        if content.contains(&ref1)
                            || content.contains(&ref2)
                            || content.contains(&ref3)
                            || content.contains(&ref4)
                            || content.contains(&ref5)
                            || content.contains(&camel1)
                            || content.contains(&camel2)
                        {
                            deps.insert(other.clone());
                        }
                    }
                }
                adj.insert(proj_name.clone(), (dir.clone(), deps));
            }
        }
    }

    let scope_label = if root_cargo.exists() {
        "crates"
    } else {
        "modules"
    };
    build_graph_report(scope_label, workspace_root, adj)
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
                if matches!(ext, "rs" | "go" | "py" | "ts" | "js" | "java" | "kt" | "cs" | "swift") {
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
                    "go" => parse_go_imports(&content, go_module_name.as_deref(), &file_modules, &mut deps),
                    "py" => parse_python_imports(&content, &file_modules, &mut deps),
                    "ts" | "js" => parse_ts_imports(&content, &file_modules, &mut deps),
                    "java" | "kt" => parse_java_imports(&content, &file_modules, &mut deps),
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

fn extract_maven_artifact_id(pom_content: &str) -> Option<String> {
    let search_content = if let Some(parent_end) = pom_content.find("</parent>") {
        &pom_content[parent_end + "</parent>".len()..]
    } else {
        pom_content
    };
    if let Some(start) = search_content.find("<artifactId>") {
        let after_start = &search_content[start + "<artifactId>".len()..];
        if let Some(end) = after_start.find("</artifactId>") {
            return Some(after_start[..end].trim().to_string());
        }
    }
    None
}

fn extract_gradle_project_block(root_content: &str, project_name: &str) -> Option<String> {
    let alt_name = project_name.replace(':', "-");
    let last_name = project_name.split(':').last().unwrap_or(project_name);
    let patterns = [
        format!("project(':{project_name}')"),
        format!("project(\":{project_name}\")"),
        format!("project('{project_name}')"),
        format!("project(\"{project_name}\")"),
        format!("project(':{alt_name}')"),
        format!("project(\":{alt_name}\")"),
        format!("project(':{last_name}')"),
        format!("project(\":{last_name}\")"),
    ];

    let mut start_idx = None;
    for pat in &patterns {
        if let Some(pos) = root_content.find(pat) {
            start_idx = Some(pos + pat.len());
            break;
        }
    }

    let start_search = start_idx?;
    let brace_offset = root_content[start_search..].find('{')?;
    let brace_start = start_search + brace_offset;

    let mut depth = 0;
    let mut end_idx = None;
    for (i, c) in root_content[brace_start..].char_indices() {
        if c == '{' {
            depth += 1;
        } else if c == '}' {
            depth -= 1;
            if depth == 0 {
                end_idx = Some(brace_start + i + 1);
                break;
            }
        }
    }

    end_idx.map(|end| root_content[brace_start..end].to_string())
}

/// Helper to convert kebab-case or snake_case identifiers to camelCase (for Gradle Type-Safe Project Accessors)
pub fn kebab_to_camel(s: &str) -> String {
    let mut result = String::new();
    let mut capitalize_next = false;
    for c in s.chars() {
        if c == '-' || c == '_' {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}


fn file_to_module_name(rel_path: &str) -> String {
    let p = if let Some(idx) = rel_path.rfind('.') {
        &rel_path[..idx]
    } else {
        rel_path
    };
    p.replace('/', "::")
}

fn parse_java_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            let rest = trimmed
                .trim_start_matches("import ")
                .trim_start_matches("static ")
                .trim_end_matches(';')
                .trim();
            let as_colons = rest.replace('.', "::");
            let class_name = rest.rsplit('.').next().unwrap_or("");
            for mod_name in known_modules.keys() {
                if mod_name.ends_with(&as_colons) || (!class_name.is_empty() && mod_name.ends_with(&format!("::{class_name}"))) {
                    deps.insert(mod_name.clone());
                }
            }
        }
    }
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

fn extract_quoted_string(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(&line[start..end])
}

fn parse_go_imports(
    content: &str,
    go_module_prefix: Option<&str>,
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
            if let Some(pkg) = extract_quoted_string(trimmed) {
                // Only resolve internal workspace packages
                let target_pkg = if let Some(prefix) = go_module_prefix {
                    if pkg == prefix {
                        Some("")
                    } else if let Some(rel) = pkg.strip_prefix(&format!("{prefix}/")) {
                        Some(rel)
                    } else {
                        None
                    }
                } else if let Some(rel) = pkg.strip_prefix("./") {
                    Some(rel)
                } else if let Some(rel) = pkg.strip_prefix("../") {
                    Some(rel)
                } else {
                    None
                };

                if let Some(target_dir) = target_pkg {
                    let target_colon = target_dir.replace('/', "::");
                    for mod_name in known_modules.keys() {
                        if target_colon.is_empty() {
                            if !mod_name.contains("::") {
                                deps.insert(mod_name.clone());
                            }
                        } else if mod_name.starts_with(&format!("{target_colon}::")) {
                            deps.insert(mod_name.clone());
                        }
                    }
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
            if let Some(path) = extract_quoted_string(trimmed) {
                // Only local relative imports: ./ or ../
                if path.starts_with('.') {
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

pub const MAX_CYCLES_DETECTED: usize = 100;

/// Detects all cycles using depth-first search with recursion stack, bounded to MAX_CYCLES_DETECTED.
pub fn find_cycles(adj: &BTreeMap<String, (PathBuf, BTreeSet<String>)>) -> Vec<Vec<String>> {
    let mut cycles = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut on_stack: HashSet<String> = HashSet::new();
    let mut current_path: Vec<String> = Vec::new();

    for start_node in adj.keys() {
        if cycles.len() >= MAX_CYCLES_DETECTED {
            break;
        }
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
    if cycles.len() >= MAX_CYCLES_DETECTED {
        return;
    }
    visited.insert(u.to_string());
    on_stack.insert(u.to_string());
    path.push(u.to_string());

    if let Some((_, neighbors)) = adj.get(u) {
        for v in neighbors {
            if cycles.len() >= MAX_CYCLES_DETECTED {
                break;
            }
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
        let shown = report.cycles.len().min(25);
        out.push_str(&format!(
            "\n🚨 CYCLES DETECTED: {} circular dependency path(s) found:\n",
            report.cycles_detected
        ));
        for (i, cycle) in report.cycles.iter().take(shown).enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, cycle.join(" -> ")));
        }
        if report.cycles_detected > shown {
            out.push_str(&format!(
                "  … and {} more circular dependency path(s) truncated\n",
                report.cycles_detected - shown
            ));
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
    fn test_cycle_detection_capped() {
        let mut adj = BTreeMap::new();
        let dummy = PathBuf::from("test");

        // 150 independent 2-cycles: A_i -> B_i -> A_i
        for i in 0..150 {
            let a = format!("A_{i}");
            let b = format!("B_{i}");
            adj.insert(a.clone(), (dummy.clone(), [b.clone()].into()));
            adj.insert(b.clone(), (dummy.clone(), [a.clone()].into()));
        }

        let cycles = find_cycles(&adj);
        assert_eq!(cycles.len(), MAX_CYCLES_DETECTED);
    }

    #[test]
    fn test_format_dependency_report_truncation() {
        let mut cycles = Vec::new();
        for i in 0..30 {
            cycles.push(vec![format!("mod{i}"), format!("mod{}", i + 1), format!("mod{i}")]);
        }
        let report = DependencyGraphReport {
            scope: "modules".to_string(),
            total_nodes: 30,
            total_edges: 60,
            cycles_detected: cycles.len(),
            cycles,
            nodes: vec![],
            isolated_nodes: vec![],
        };
        let formatted = format_dependency_report(&report);
        assert!(formatted.contains("25. mod24 -> mod25 -> mod24"));
        assert!(formatted.contains("… and 5 more circular dependency path(s) truncated"));
    }

    #[test]
    fn test_parse_go_imports_internal_matching() {
        let content = r#"
package main

import (
    "fmt"
    "net/http"
    "github.com/example/app/pkg/util"
    ext "github.com/other/lib"
)
"#;
        let mut known = HashMap::new();
        known.insert("pkg::util::helper".to_string(), PathBuf::from("pkg/util/helper.go"));
        known.insert("http::server".to_string(), PathBuf::from("http/server.go"));

        let mut deps = BTreeSet::new();
        parse_go_imports(content, Some("github.com/example/app"), &known, &mut deps);

        assert!(deps.contains("pkg::util::helper"));
        assert!(!deps.contains("http::server"));
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

    #[test]
    fn test_extract_maven_artifact_id() {
        let pom_with_parent = r#"
<project>
  <parent>
    <groupId>io.netty</groupId>
    <artifactId>netty-parent</artifactId>
    <version>4.2.19</version>
  </parent>
  <artifactId>netty-buffer</artifactId>
</project>
"#;
        assert_eq!(extract_maven_artifact_id(pom_with_parent), Some("netty-buffer".to_string()));

        let pom_without_parent = r#"
<project>
  <groupId>org.example</groupId>
  <artifactId>my-module</artifactId>
</project>
"#;
        assert_eq!(extract_maven_artifact_id(pom_without_parent), Some("my-module".to_string()));
    }

    #[test]
    fn test_kebab_to_camel() {
        assert_eq!(kebab_to_camel("ktor-utils"), "ktorUtils");
        assert_eq!(kebab_to_camel("ktor-server-test-suites"), "ktorServerTestSuites");
        assert_eq!(kebab_to_camel("my_module_name"), "myModuleName");
        assert_eq!(kebab_to_camel("simple"), "simple");
    }
}
