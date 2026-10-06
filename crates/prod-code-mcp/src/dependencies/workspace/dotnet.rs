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

pub fn collect_dotnet_dependencies(
    workspace_root: &Path,
    adj: &mut BTreeMap<String, (PathBuf, BTreeSet<String>)>,
) {
    if !adj.is_empty() {
        return;
    }

    let has_csharp = workspace_root.join("global.json").exists()
        || workspace_root.join("Directory.Build.props").exists()
        || workspace_root.join("Directory.Build.targets").exists();
    let walker = WalkBuilder::new(workspace_root)
        .hidden(true)
        .git_ignore(true)
        .max_depth(Some(8))
        .build();
    let mut csproj_files = Vec::new();
    for entry in walker.flatten() {
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("csproj") {
            let path_str = path.to_string_lossy();
            if !path_str.contains("/bin/")
                && !path_str.contains("/obj/")
                && !path_str.contains("/.git/")
                && !path_str.contains("/artifacts/")
                && !path_str.contains("\\bin\\")
                && !path_str.contains("\\obj\\")
                && !path_str.contains("\\.git\\")
                && !path_str.contains("\\artifacts\\")
            {
                csproj_files.push(path.to_path_buf());
            }
        }
    }

    if has_csharp || !csproj_files.is_empty() {
        let mut project_map = HashMap::new();
        for csproj in &csproj_files {
            if let Some(stem) = csproj.file_stem().and_then(|s| s.to_str()) {
                let dir = csproj.parent().unwrap_or(workspace_root).to_path_buf();
                let content = std::fs::read_to_string(csproj).unwrap_or_default();
                project_map.insert(stem.to_string(), (dir, content));
            }
        }

        for (proj_name, (dir, content)) in &project_map {
            let mut deps = BTreeSet::new();
            for other in project_map.keys() {
                if other != proj_name {
                    let ref1 = format!("Include=\"{other}\"");
                    let ref2 = format!("Include=\"{other}.csproj\"");
                    let ref3 = format!("/{other}.csproj\"");
                    let ref4 = format!("\\{other}.csproj\"");
                    if content.contains(&ref1)
                        || content.contains(&ref2)
                        || content.contains(&ref3)
                        || content.contains(&ref4)
                    {
                        deps.insert(other.clone());
                    }
                }
            }
            adj.insert(proj_name.clone(), (dir.clone(), deps));
        }
    }
}
