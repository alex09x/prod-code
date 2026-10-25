/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::collections::BTreeSet;
use std::path::PathBuf;

pub(super) fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

pub(super) fn graph(
    root: &Path,
    rows: &[(&str, &str, &[&str])],
) -> BTreeMap<String, (PathBuf, BTreeSet<String>)> {
    rows.iter()
        .map(|(name, path, deps)| {
            (
                name.to_string(),
                (
                    root.join(path),
                    deps.iter().map(|s| s.to_string()).collect(),
                ),
            )
        })
        .collect()
}

#[test]
fn reports_cargo_graph_with_exact_coupling_and_relative_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = ['client', 'core']\n",
    );
    write(
        root,
        "client/Cargo.toml",
        "[package]\nname = 'client'\n[dependencies]\ncore = '1'\n",
    );
    write(root, "core/Cargo.toml", "[package]\nname = 'core'\n");
    let report = analyze_crate_dependencies(root, Some(Path::new("unused"))).unwrap();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        serde_json::json!({
            "scope": "crates", "total_nodes": 2, "total_edges": 1,
            "cycles_detected": 0, "cycles": [], "isolated_nodes": [],
            "nodes": [
                {"name": "core", "path": "core", "afferent_coupling": 1, "efferent_coupling": 0, "instability": 0.0, "dependencies": []},
                {"name": "client", "path": "client", "afferent_coupling": 0, "efferent_coupling": 1, "instability": 1.0, "dependencies": ["core"]}
            ]
        })
    );
}

#[test]
fn reports_non_cargo_workspace_and_propagates_manifest_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    assert_eq!(
        serde_json::to_value(analyze_crate_dependencies(root, None).unwrap()).unwrap(),
        serde_json::json!({
            "scope": "modules", "total_nodes": 0, "total_edges": 0, "cycles_detected": 0,
            "cycles": [], "nodes": [], "isolated_nodes": []
        })
    );
    write(root, "Library.csproj", "<Project />");
    let report = analyze_crate_dependencies(root, None).unwrap();
    assert_eq!(report.scope, "modules");
    assert_eq!(report.total_nodes, 1);
    assert_eq!(report.isolated_nodes, ["Library"]);
    assert_eq!(report.nodes[0].path, "");
    write(root, "Cargo.toml", "[bad");
    assert_eq!(
        analyze_crate_dependencies(root, None)
            .unwrap_err()
            .to_string(),
        "Failed to parse root Cargo.toml"
    );
}
