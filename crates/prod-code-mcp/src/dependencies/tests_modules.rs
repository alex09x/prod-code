/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Module-graph scan over a temporary checkout, the scope dispatcher and the text report.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::modules::{analyze_module_dependencies, build_graph_report, file_to_module_name};
use super::{
    DependencyGraphReport, DependencyNode, DependencyScope, analyze_dependencies,
    format_dependency_report,
};

fn checkout() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("depsgraph")
        .tempdir()
        .unwrap()
}

fn write(root: &Path, rel: &str, content: impl AsRef<[u8]>) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn node<'a>(report: &'a DependencyGraphReport, name: &str) -> &'a DependencyNode {
    report.nodes.iter().find(|n| n.name == name).unwrap()
}

#[test]
fn module_names_replace_separators_and_drop_the_last_extension() {
    for (rel, want) in [
        ("src/lib.rs", "src::lib"),
        ("main.go", "main"),
        ("a.b/c.rs", "a.b::c"),
        ("pkg/util.test.ts", "pkg::util.test"),
        ("noext", "noext"),
    ] {
        assert_eq!(file_to_module_name(rel), want, "{rel}");
    }
}

#[test]
fn module_scan_resolves_every_language_and_reports_coupling() {
    let dir = checkout();
    let root = dir.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(root, ".gitignore", "gen/\n");
    write(root, "gen/skipped.rs", "mod core;\n");
    write(root, ".hidden/h.rs", "mod core;\n");
    write(root, "README.md", "import py.lib\n");
    write(root, "go.mod", "module example.org/app\n\ngo 1.21\n");
    write(
        root,
        "svc/main.go",
        "package main\n\nimport (\n\t\"fmt\"\n\t\"example.org/app/store\"\n)\n",
    );
    write(root, "store/db.go", "package store\n");
    write(root, "src/core.rs", "mod util;\nuse crate::util::helper;\n");
    write(
        root,
        "src/util.rs",
        "mod util;\npub use crate::core::run;\n",
    );
    write(root, "web/page.ts", "import { v } from './view';\n");
    write(root, "web/view.js", "export const v = 1;\n");
    write(root, "py/run.py", "import py.lib\n");
    write(root, "py/lib.py", "");
    write(root, "py/bin.py", [0xff, 0xfe, 0x00]);
    write(root, "jv/A.java", "import com.acme.B;\n");
    write(root, "jv/com/acme/B.java", "package com.acme;\n");
    write(root, "kt/K.kt", "import com.acme.B\n");
    write(root, "Prog.cs", "using Lib;\n");
    write(root, "Lib.cs", "namespace Lib {}\n");
    write(root, "solo/alone.rs", "");

    let report = analyze_module_dependencies(root, None).unwrap();

    assert_eq!(report.scope, "modules");
    assert_eq!(report.total_nodes, 15);
    assert_eq!(report.total_edges, 8);
    assert_eq!(report.cycles_detected, 1);
    assert_eq!(
        report.cycles,
        vec![vec!["src::core", "src::util", "src::core"]]
    );
    assert_eq!(report.isolated_nodes, vec!["py::bin", "solo::alone"]);

    // The most depended-on module comes first.
    assert_eq!(report.nodes[0].name, "jv::com::acme::B");
    let expected_edges = [
        ("Prog", "Lib"),
        ("jv::A", "jv::com::acme::B"),
        ("kt::K", "jv::com::acme::B"),
        ("py::run", "py::lib"),
        ("src::core", "src::util"),
        ("src::util", "src::core"),
        ("svc::main", "store::db"),
        ("web::page", "web::view"),
    ];
    for (from, to) in expected_edges {
        assert_eq!(node(&report, from).dependencies, vec![to], "{from}");
    }
    let b = node(&report, "jv::com::acme::B");
    assert_eq!(
        (b.afferent_coupling, b.efferent_coupling, b.instability),
        (2, 0, 0.0)
    );
    assert_eq!(b.path, "jv/com/acme/B.java");
    let core = node(&report, "src::core");
    assert_eq!(
        (
            core.afferent_coupling,
            core.efferent_coupling,
            core.instability
        ),
        (1, 1, 0.5)
    );
    assert_eq!(core.path, "src/core.rs");
    assert!(node(&report, "py::bin").dependencies.is_empty());
}

#[test]
fn module_scan_limits_to_target_path_and_names_modules_from_the_workspace_root() {
    let dir = checkout();
    let root = dir.path();
    write(root, "go.mod", "go 1.21\n");
    write(root, "other.rs", "mod a;\n");
    write(root, "sub/a.rs", "mod b;\n");
    write(root, "sub/b.rs", "");

    let report =
        analyze_dependencies(root, DependencyScope::Modules, Some(&root.join("sub"))).unwrap();

    assert_eq!(report.scope, "modules");
    assert_eq!(report.total_nodes, 2);
    assert_eq!(report.total_edges, 1);
    assert_eq!(node(&report, "sub::a").dependencies, vec!["sub::b"]);
    assert_eq!(node(&report, "sub::b").afferent_coupling, 1);
}

#[test]
fn module_scan_without_supported_sources_is_refused() {
    let dir = checkout();
    write(dir.path(), "notes.md", "nothing to scan\n");
    let err = analyze_module_dependencies(dir.path(), None).unwrap_err();
    assert_eq!(
        err.to_string(),
        "dependency graph analysis found no supported source modules; supported languages are Rust, Go, Python, TypeScript/JavaScript, Java/Kotlin, and C#"
    );
}

#[test]
fn crate_scope_reads_cargo_members_and_defaults_to_crates() {
    assert_eq!(DependencyScope::default(), DependencyScope::Crates);
    let dir = checkout();
    let root = dir.path();
    write(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write(
        root,
        "crates/alpha/Cargo.toml",
        "[package]\nname = \"alpha\"\nversion = \"0.1.0\"\n\n[dependencies]\nbeta = { path = \"../beta\" }\n",
    );
    write(
        root,
        "crates/beta/Cargo.toml",
        "[package]\nname = \"beta\"\nversion = \"0.1.0\"\n",
    );

    let report = analyze_dependencies(root, DependencyScope::default(), None).unwrap();

    assert_eq!(report.scope, "crates");
    assert_eq!((report.total_nodes, report.total_edges), (2, 1));
    assert_eq!(report.nodes[0].name, "beta");
    assert_eq!(node(&report, "alpha").dependencies, vec!["beta"]);
    assert_eq!(node(&report, "alpha").instability, 1.0);

    let empty = checkout();
    let report = analyze_dependencies(empty.path(), DependencyScope::Crates, None).unwrap();
    assert_eq!(report.scope, "modules");
    assert_eq!(report.total_nodes, 0);
}

#[test]
fn graph_report_counts_cycles_isolated_nodes_and_relative_paths() {
    let root = Path::new("/ws");
    let mut adj: BTreeMap<String, (PathBuf, BTreeSet<String>)> = BTreeMap::new();
    adj.insert(
        "a".into(),
        (PathBuf::from("/ws/a.rs"), ["b".to_string()].into()),
    );
    adj.insert(
        "b".into(),
        (PathBuf::from("/ws/dir/b.rs"), ["a".to_string()].into()),
    );
    adj.insert(
        "c".into(),
        (PathBuf::from("/elsewhere/c.rs"), BTreeSet::new()),
    );

    let report = build_graph_report("modules", root, adj).unwrap();

    assert_eq!((report.total_nodes, report.total_edges), (3, 2));
    assert_eq!(report.cycles, vec![vec!["a", "b", "a"]]);
    assert_eq!(report.isolated_nodes, vec!["c"]);
    let paths: Vec<&str> = report.nodes.iter().map(|n| n.path.as_str()).collect();
    assert_eq!(paths, vec!["a.rs", "dir/b.rs", "/elsewhere/c.rs"]);
    assert_eq!(node(&report, "c").instability, 0.0);
    assert_eq!(node(&report, "a").instability, 0.5);
}

fn report(nodes: Vec<DependencyNode>, isolated: Vec<String>) -> DependencyGraphReport {
    DependencyGraphReport {
        scope: "crates".to_string(),
        total_nodes: nodes.len() + isolated.len(),
        total_edges: 1,
        cycles_detected: 0,
        cycles: vec![],
        nodes,
        isolated_nodes: isolated,
    }
}

fn coupled(name: &str, ca: usize, ce: usize, instability: f32) -> DependencyNode {
    DependencyNode {
        name: name.to_string(),
        path: format!("{name}.rs"),
        afferent_coupling: ca,
        efferent_coupling: ce,
        instability,
        dependencies: vec![],
    }
}

#[test]
fn format_clean_graph_prints_table_and_isolated_nodes() {
    let text = format_dependency_report(&report(
        vec![coupled("a", 1, 0, 0.0), coupled("b", 0, 1, 1.0)],
        vec!["c".to_string()],
    ));
    let expected = [
        "⚡ prod-code Architecture & Dependency Graph Report",
        "────────────────────────────────────────────────────",
        "Scope: crates | Nodes: 3 | Dependencies: 1",
        "",
        "✓ Zero circular dependencies detected. Architecture graph is a clean DAG.",
        "",
        "Top Coupled Modules / Crates (by Afferent Coupling Ca):",
        "  Name                                Ca    Ce  Instab",
        "  ────────────────────────────────────────────────────",
        "  a                                    1     0    0.00",
        "  b                                    0     1    1.00",
        "",
        "Isolated (Leaf/Orphan) Nodes (1): c",
        "",
    ]
    .join("\n");
    assert_eq!(text, expected);
}

#[test]
fn format_lists_cycles_without_truncation_note_and_caps_tables() {
    let nodes: Vec<DependencyNode> = (0..20)
        .map(|i| coupled(&format!("n{i:02}"), 0, 0, 0.0))
        .collect();
    let isolated: Vec<String> = (0..12).map(|i| format!("iso{i}")).collect();
    let mut graph = report(nodes, isolated);
    graph.cycles = vec![vec!["a".into(), "b".into(), "a".into()]];
    graph.cycles_detected = 1;

    let text = format_dependency_report(&graph);

    assert!(text.contains(
        "\n🚨 CYCLES DETECTED: 1 circular dependency path(s) found:\n  1. a -> b -> a\n\nTop Coupled"
    ));
    assert!(!text.contains("truncated"));
    assert!(!text.contains("Zero circular"));
    assert!(text.contains("  n14 "));
    assert!(!text.contains("n15"));
    assert!(text.ends_with(
        "Isolated (Leaf/Orphan) Nodes (12): iso0, iso1, iso2, iso3, iso4, iso5, iso6, iso7, iso8, iso9\n"
    ));
}

#[test]
fn format_without_isolated_nodes_omits_the_section() {
    let text = format_dependency_report(&report(vec![coupled("a", 1, 1, 0.5)], vec![]));
    assert!(text.ends_with("    0.50\n"));
    assert!(!text.contains("Isolated"));
}
