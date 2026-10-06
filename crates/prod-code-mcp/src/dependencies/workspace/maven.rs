/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::helpers::extract_maven_artifact_id;

pub fn collect_maven_dependencies(
    workspace_root: &Path,
    adj: &mut BTreeMap<String, (PathBuf, BTreeSet<String>)>,
) {
    let root_pom = workspace_root.join("pom.xml");
    if !root_pom.exists() {
        return;
    }
    let Ok(pom_content) = std::fs::read_to_string(&root_pom) else {
        return;
    };

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
            if target_mod != mod_name
                && content.contains(&format!("<artifactId>{art}</artifactId>"))
            {
                deps.insert(target_mod.clone());
            }
        }
        adj.insert(mod_name.clone(), (dir.clone(), deps));
    }
}
