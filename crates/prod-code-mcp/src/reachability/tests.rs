/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::classify::is_root_entry_point;
use super::graph::ReachabilityGraph;
use super::types::{SymbolDecl, SymbolKey};

#[test]
fn test_is_root_entry_point_classification() {
    // main and init are roots
    assert!(is_root_entry_point("rust", "src/main.rs", "main", "function", false, false).0);
    assert!(is_root_entry_point("go", "pkg/init.go", "init", "function", false, false).0);

    // Exported symbols are roots in library mode (include_exported = false)
    assert!(is_root_entry_point("rust", "src/lib.rs", "compute", "function", true, false).0);
    // Exported symbols are NOT roots when include_exported = true
    assert!(!is_root_entry_point("rust", "src/lib.rs", "compute", "function", true, true).0);

    // Trait methods are roots (dynamic dispatch)
    assert!(is_root_entry_point("rust", "src/lib.rs", "draw", "trait-method", false, true).0);

    // Test functions are roots
    assert!(
        is_root_entry_point(
            "rust",
            "src/lib.rs",
            "test_addition",
            "function",
            false,
            true
        )
        .0
    );
}

#[test]
fn test_circular_dead_cycle_detection() {
    let mut graph = ReachabilityGraph::new();

    // 1. main function (root entry point)
    let main_decl = SymbolDecl {
        key: SymbolKey::new("src/main.rs", "main", 1, 1),
        kind: "function".to_string(),
        range_start: (1, 1),
        range_end: (3, 2),
        exported: false,
        is_root: true,
        root_reason: Some("main".to_string()),
    };
    let main_idx = graph.add_symbol(main_decl);

    // 2. active_fn (called by main)
    let active_decl = SymbolDecl {
        key: SymbolKey::new("src/main.rs", "active_fn", 5, 1),
        kind: "function".to_string(),
        range_start: (5, 1),
        range_end: (7, 2),
        exported: false,
        is_root: false,
        root_reason: None,
    };
    let active_idx = graph.add_symbol(active_decl);
    graph.add_edge(main_idx, active_idx);

    // 3. cycle_a calls cycle_b
    let cycle_a_decl = SymbolDecl {
        key: SymbolKey::new("src/cycle.rs", "cycle_a", 10, 1),
        kind: "function".to_string(),
        range_start: (10, 1),
        range_end: (12, 2),
        exported: false,
        is_root: false,
        root_reason: None,
    };
    let cycle_a_idx = graph.add_symbol(cycle_a_decl);

    // 4. cycle_b calls cycle_a
    let cycle_b_decl = SymbolDecl {
        key: SymbolKey::new("src/cycle.rs", "cycle_b", 15, 1),
        kind: "function".to_string(),
        range_start: (15, 1),
        range_end: (17, 2),
        exported: false,
        is_root: false,
        root_reason: None,
    };
    let cycle_b_idx = graph.add_symbol(cycle_b_decl);

    // Circular edge: cycle_a <-> cycle_b
    graph.add_edge(cycle_a_idx, cycle_b_idx);
    graph.add_edge(cycle_b_idx, cycle_a_idx);

    // 5. Compute reachability
    let result = graph.compute_reachability();

    // main and active_fn are reachable
    assert_eq!(result.summary.roots_count, 1);
    assert_eq!(result.summary.reachable_count, 2);
    assert!(
        result
            .reachable_keys
            .contains(&SymbolKey::new("src/main.rs", "main", 1, 1))
    );
    assert!(
        result
            .reachable_keys
            .contains(&SymbolKey::new("src/main.rs", "active_fn", 5, 1))
    );

    // cycle_a and cycle_b are UNREACHABLE
    assert_eq!(result.summary.unreachable_count, 2);
    let unreachable_names: Vec<&str> = result
        .unreachable_items
        .iter()
        .map(|d| d.name.as_str())
        .collect();
    assert!(unreachable_names.contains(&"cycle_a"));
    assert!(unreachable_names.contains(&"cycle_b"));

    // They form an UnreachableCluster with cycle = true
    assert_eq!(result.unreachable_clusters.len(), 1);
    let cluster = &result.unreachable_clusters[0];
    assert_eq!(cluster.symbols.len(), 2);
    assert!(cluster.cycle);
    assert_eq!(cluster.internal_calls.len(), 2);
}

#[test]
fn test_unverified_node_safety_under_contract_435() {
    let mut graph = ReachabilityGraph::new();

    // An unverified node (e.g. query failed/timed out)
    let unverified_decl = SymbolDecl {
        key: SymbolKey::new("src/lib.rs", "unverified_fn", 1, 1),
        kind: "function".to_string(),
        range_start: (1, 1),
        range_end: (3, 2),
        exported: false,
        is_root: false,
        root_reason: None,
    };
    let u_idx = graph.add_symbol(unverified_decl);
    graph.mark_unverified(u_idx);

    // helper called by unverified node
    let helper_decl = SymbolDecl {
        key: SymbolKey::new("src/lib.rs", "helper_fn", 5, 1),
        kind: "function".to_string(),
        range_start: (5, 1),
        range_end: (7, 2),
        exported: false,
        is_root: false,
        root_reason: None,
    };
    let h_idx = graph.add_symbol(helper_decl);
    graph.add_edge(u_idx, h_idx);

    let result = graph.compute_reachability();

    // helper_fn must NOT be marked dead because it is reachable from unverified node
    assert_eq!(result.summary.unreachable_count, 0);
    assert!(result.unreachable_items.is_empty());
}

#[test]
fn test_enclosing_symbol_lookup() {
    let mut graph = ReachabilityGraph::new();

    // Outer struct lines 1..20
    graph.add_symbol(SymbolDecl {
        key: SymbolKey::new("src/lib.rs", "MyStruct", 1, 8),
        kind: "struct".to_string(),
        range_start: (1, 1),
        range_end: (20, 2),
        exported: true,
        is_root: false,
        root_reason: None,
    });

    // Inner method lines 5..10
    let method_idx = graph.add_symbol(SymbolDecl {
        key: SymbolKey::new("src/lib.rs", "my_method", 5, 8),
        kind: "method".to_string(),
        range_start: (5, 1),
        range_end: (10, 2),
        exported: false,
        is_root: false,
        root_reason: None,
    });

    // Reference at line 7 should resolve to inner method
    assert_eq!(
        graph.find_enclosing_symbol("src/lib.rs", 7, 12),
        Some(method_idx)
    );

    // Reference at line 15 should resolve to outer struct
    assert_eq!(graph.find_enclosing_symbol("src/lib.rs", 15, 4), Some(0));

    // Reference outside should return None
    assert_eq!(graph.find_enclosing_symbol("src/lib.rs", 25, 1), None);
}
