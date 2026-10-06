/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ignore::WalkBuilder;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::helpers::{extract_gradle_project_block, kebab_to_camel};

pub fn collect_gradle_dependencies(
    workspace_root: &Path,
    adj: &mut BTreeMap<String, (PathBuf, BTreeSet<String>)>,
) {
    let root_gradle = workspace_root.join("settings.gradle");
    let root_gradle_kts = workspace_root.join("settings.gradle.kts");
    let gradle_settings_path = if root_gradle.exists() {
        Some(root_gradle)
    } else if root_gradle_kts.exists() {
        Some(root_gradle_kts)
    } else {
        None
    };

    let Some(settings_file) = gradle_settings_path else {
        return;
    };
    let Ok(settings_content) = std::fs::read_to_string(&settings_file) else {
        return;
    };

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
            if (!trimmed.ends_with(',') && !trimmed.starts_with("include"))
                || trimmed.ends_with(')')
            {
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
            if (rest.starts_with('"') && rest.len() >= 2)
                || (rest.starts_with('\'') && rest.len() >= 2)
            {
                let quote = rest.chars().next().unwrap();
                if let Some(end_idx) = rest[1..].find(quote) {
                    let path_str = &rest[1..1 + end_idx];
                    let mod_name = path_str
                        .split('/')
                        .next_back()
                        .unwrap_or(path_str)
                        .trim_matches(':');
                    if !mod_name.is_empty() && !projects.contains(&mod_name.to_string()) {
                        projects.push(mod_name.to_string());
                    }
                }
            }
        }

        // Handle Kotlin unaryPlus DSL: +"foo-bar" or + "foo-bar"
        if trimmed.starts_with('+') {
            let rest = trimmed.trim_start_matches('+').trim();
            if (rest.starts_with('"') && rest.len() >= 2)
                || (rest.starts_with('\'') && rest.len() >= 2)
            {
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
            if (file_name == "build.gradle" || file_name == "build.gradle.kts")
                && let Some(parent) = path.parent()
            {
                let rel_dir = parent.strip_prefix(workspace_root).unwrap_or(parent);
                let rel_str = rel_dir.to_string_lossy().to_string();
                if !rel_str.is_empty()
                    && !rel_str.starts_with("build-")
                    && !rel_str.starts_with(".gradle")
                    && !rel_str.starts_with("build/")
                    && !rel_str.starts_with("buildSrc")
                    && let Some(folder_name) = parent.file_name().and_then(|n| n.to_str())
                {
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
        } else if let Some(last) = p.split(':').next_back() {
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
                    other
                        .split(':')
                        .map(kebab_to_camel)
                        .collect::<Vec<_>>()
                        .join(".")
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
