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

use super::cycles::find_cycles;
use super::format::format_dependency_report;
use super::languages::{parse_csharp_imports, parse_go_imports};
use super::modules::build_graph_report;
use super::types::{DependencyGraphReport, MAX_CYCLES_DETECTED};
use super::workspace::{extract_maven_artifact_id, kebab_to_camel};

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
        cycles.push(vec![
            format!("mod{i}"),
            format!("mod{}", i + 1),
            format!("mod{i}"),
        ]);
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
    known.insert(
        "pkg::util::helper".to_string(),
        PathBuf::from("pkg/util/helper.go"),
    );
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

    adj.insert(
        "A".to_string(),
        (dummy.clone(), ["B".to_string(), "C".to_string()].into()),
    );
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
    assert_eq!(
        extract_maven_artifact_id(pom_with_parent),
        Some("netty-buffer".to_string())
    );

    let pom_without_parent = r#"
<project>
  <groupId>org.example</groupId>
  <artifactId>my-module</artifactId>
</project>
"#;
    assert_eq!(
        extract_maven_artifact_id(pom_without_parent),
        Some("my-module".to_string())
    );
}

#[test]
fn test_kebab_to_camel() {
    assert_eq!(kebab_to_camel("ktor-utils"), "ktorUtils");
    assert_eq!(
        kebab_to_camel("ktor-server-test-suites"),
        "ktorServerTestSuites"
    );
    assert_eq!(kebab_to_camel("my_module_name"), "myModuleName");
    assert_eq!(kebab_to_camel("simple"), "simple");
}

#[test]
fn test_parse_csharp_imports() {
    let content = r#"
using System;
using System.Collections.Generic;
using Microsoft.AspNetCore.Http;
using CustomAlias = Microsoft.AspNetCore.Routing.Router;
using static Microsoft.AspNetCore.Hosting.Constants;
"#;
    let mut known = HashMap::new();
    known.insert(
        "Microsoft::AspNetCore::Http".to_string(),
        PathBuf::from("Http.cs"),
    );
    known.insert(
        "Microsoft::AspNetCore::Routing::Router".to_string(),
        PathBuf::from("Router.cs"),
    );
    known.insert("Unrelated::Module".to_string(), PathBuf::from("Other.cs"));

    let mut deps = BTreeSet::new();
    parse_csharp_imports(content, &known, &mut deps);
    assert!(deps.contains("Microsoft::AspNetCore::Http"));
    assert!(deps.contains("Microsoft::AspNetCore::Routing::Router"));
    assert!(!deps.contains("Unrelated::Module"));
}
