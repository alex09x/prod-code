/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Whole-program graph reachability analysis from entry points (roadmap 7.5, 8.6).
//!
//! Identifies unreachable functions, types, and circular dead cycles (e.g. `cycle_a <-> cycle_b`)
//! that simple reference counting misses.
//!
//! Traversal starts from root entry points:
//! 1. Application entry points (`main`, `init`, binaries).
//! 2. Library public API entry points (exported symbols when in library mode `include_exported: false`).
//! 3. Test entry points and dynamic dispatch candidates (`trait-method`, interfaces).
//!
//! Query failures/timeouts are recorded as unverified and treated safely per contract #435:
//! an unverified symbol is never assumed unreachable and its dependencies are preserved.

use crate::dead_code::{DeadItem, Unverified};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

/// A key uniquely identifying a symbol declaration within a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SymbolKey {
    pub file: String,
    pub name: String,
    pub line: u32,
    pub col: u32,
}

impl SymbolKey {
    pub fn new(file: impl Into<String>, name: impl Into<String>, line: u32, col: u32) -> Self {
        Self {
            file: file.into(),
            name: name.into(),
            line,
            col,
        }
    }
}

/// A symbol declaration with its AST range, export status, and entry point classification.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolDecl {
    pub key: SymbolKey,
    pub kind: String,
    /// 1-based start (line, col) of the declaration body/span.
    pub range_start: (u32, u32),
    /// 1-based end (line, col) of the declaration body/span.
    pub range_end: (u32, u32),
    pub exported: bool,
    pub is_root: bool,
    pub root_reason: Option<String>,
}

/// A cluster of unreachable symbols that call or reference each other,
/// forming a dead subgraph or circular dead cycle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UnreachableCluster {
    pub symbols: Vec<DeadItem>,
    /// Whether there is a circular dependency (call cycle) within the cluster.
    pub cycle: bool,
    /// Internal directed call/reference edges within the cluster: (caller, callee).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub internal_calls: Vec<(String, String)>,
}

/// Summary metrics of whole-program reachability analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReachabilitySummary {
    pub roots_count: usize,
    pub reachable_count: usize,
    pub unreachable_count: usize,
    pub cluster_count: usize,
}

/// The result of whole-program graph reachability analysis.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReachabilityResult {
    pub summary: ReachabilitySummary,
    pub roots: Vec<DeadItem>,
    pub reachable_keys: Vec<SymbolKey>,
    pub unreachable_items: Vec<DeadItem>,
    pub unreachable_clusters: Vec<UnreachableCluster>,
    pub unverified: Vec<Unverified>,
}

/// Determines whether a symbol is a root entry point based on name, language conventions,
/// file path, and export status.
pub fn is_root_entry_point(
    language: &str,
    rel_path: &str,
    name: &str,
    kind: &str,
    exported: bool,
    include_exported: bool,
) -> (bool, Option<String>) {
    let bare = name.split('(').next().unwrap_or(name);

    // 1. Universal entry point function names
    if matches!(bare, "main" | "init") {
        return (true, Some(format!("entry-point function `{bare}`")));
    }

    // 2. Trait and interface implementations (dynamic dispatch candidates)
    if kind == "trait-method" || (kind == "method" && language != "rust") {
        return (
            true,
            Some("trait or interface implementation (dynamic dispatch)".to_string()),
        );
    }

    // 3. Known lifecycle / standard methods
    if matches!(
        bare,
        "new" | "default" | "drop" | "fmt" | "eq" | "hash" | "clone"
    ) || bare.starts_with("__")
    {
        return (
            true,
            Some(format!("standard lifecycle/protocol hook `{bare}`")),
        );
    }

    // 4. Test entry points
    if bare.starts_with("test") || bare.starts_with("Test") {
        return (true, Some(format!("test entry point `{bare}`")));
    }

    // 5. Entry script conventions (for module/script entry points in dynamic languages)
    let lower_path = rel_path.to_ascii_lowercase();
    let is_entry_script = match language {
        "python" => lower_path.ends_with("/__main__.py") || lower_path == "__main__.py",
        "typescript" | "javascript" => {
            (lower_path == "index.ts"
                || lower_path == "index.js"
                || lower_path == "main.ts"
                || lower_path == "main.js"
                || lower_path == "server.ts"
                || lower_path == "server.js")
                && exported
        }
        _ => false,
    };

    if is_entry_script {
        return (
            true,
            Some(format!("entry script item in `{rel_path}`")),
        );
    }

    // 6. Library mode: when `include_exported == false`, all exported items are library entry points
    if exported && !include_exported {
        return (
            true,
            Some("exported public API (library root entry point)".to_string()),
        );
    }

    (false, None)
}

/// A directed graph representing symbols and their reference / call dependencies.
#[derive(Default)]
pub struct ReachabilityGraph {
    pub symbols: Vec<SymbolDecl>,
    pub key_to_idx: HashMap<SymbolKey, usize>,
    /// File to symbol indices sorted by range start.
    pub file_symbols: HashMap<String, Vec<usize>>,
    /// Directed edges: u -> v means u references or calls v.
    pub outgoing_edges: HashMap<usize, HashSet<usize>>,
    /// Directed edges: v -> u means u references or calls v.
    pub incoming_edges: HashMap<usize, HashSet<usize>>,
    /// Indices of symbols with unverified query status.
    pub unverified_indices: HashSet<usize>,
}

impl ReachabilityGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a symbol declaration to the graph.
    pub fn add_symbol(&mut self, decl: SymbolDecl) -> usize {
        let idx = self.symbols.len();
        self.key_to_idx.insert(decl.key.clone(), idx);
        self.file_symbols
            .entry(decl.key.file.clone())
            .or_default()
            .push(idx);
        self.symbols.push(decl);
        idx
    }

    /// Marks a symbol as having unverified status (query failed, null, or malformed).
    pub fn mark_unverified(&mut self, idx: usize) {
        self.unverified_indices.insert(idx);
    }

    /// Adds a directed dependency edge: `from` references / calls `to`.
    pub fn add_edge(&mut self, from: usize, to: usize) {
        if from != to {
            self.outgoing_edges.entry(from).or_default().insert(to);
            self.incoming_edges.entry(to).or_default().insert(from);
        }
    }

    /// Finds the most specific enclosing symbol declaration in `file` at `(line, col)`.
    pub fn find_enclosing_symbol(&self, file: &str, line: u32, col: u32) -> Option<usize> {
        let indices = self.file_symbols.get(file)?;
        let mut best: Option<(usize, u64)> = None;

        for &idx in indices {
            let decl = &self.symbols[idx];
            let (sl, sc) = decl.range_start;
            let (el, ec) = decl.range_end;

            let after_start = line > sl || (line == sl && col >= sc);
            let before_end = line < el || (line == el && col <= ec);

            if after_start && before_end {
                // Calculate span size to select the innermost / smallest enclosing declaration
                let span_lines = (el.saturating_sub(sl)) as u64;
                let span_chars = (ec as i64 - sc as i64).unsigned_abs();
                let span = span_lines * 10_000 + span_chars;

                match best {
                    None => best = Some((idx, span)),
                    Some((_, best_span)) if span < best_span => best = Some((idx, span)),
                    _ => {}
                }
            }
        }

        best.map(|(idx, _)| idx)
    }

    /// Records reference locations for candidate `target_idx`, establishing caller/user edges.
    pub fn record_references(
        &mut self,
        target_idx: usize,
        ref_locations: &[(String, u32, u32)],
        is_test_ref: impl Fn(&str, u32) -> bool,
    ) -> Vec<(String, u32, u32)> {
        let mut unattributed = Vec::new();
        for (ref_file, ref_line, ref_col) in ref_locations {
            if is_test_ref(ref_file, *ref_line) {
                // Called from a test; mark target reachable by treating it as referenced by a test root
                let decl = &mut self.symbols[target_idx];
                if !decl.is_root {
                    decl.is_root = true;
                    decl.root_reason = Some(format!("referenced by test in {ref_file}:{ref_line}"));
                }
                continue;
            }

            if let Some(caller_idx) = self.find_enclosing_symbol(ref_file, *ref_line, *ref_col) {
                self.add_edge(caller_idx, target_idx);
            } else {
                // A real reference with no indexed enclosing declaration may come from
                // module-level code or a file omitted from the scan. Do not call its target dead.
                self.mark_unverified(target_idx);
                unattributed.push((ref_file.clone(), *ref_line, *ref_col));
            }
        }
        unattributed
    }

    /// Computes whole-program graph reachability from all root entry points.
    pub fn compute_reachability(&self) -> ReachabilityResult {
        let mut reachable = HashSet::new();
        let mut queue = VecDeque::new();
        let mut roots = Vec::new();

        // 1. Seed traversal with all root entry points
        for (idx, decl) in self.symbols.iter().enumerate() {
            if decl.is_root {
                reachable.insert(idx);
                queue.push_back(idx);
                roots.push(DeadItem {
                    name: decl.key.name.clone(),
                    kind: decl.kind.clone(),
                    file: decl.key.file.clone(),
                    line: decl.key.line,
                    col: decl.key.col,
                    exported: decl.exported,
                });
            }
        }

        // 2. Safety under #435: Treat unverified nodes conservatively as potential roots so
        // their transitive callees are not falsely declared dead.
        for &unverified_idx in &self.unverified_indices {
            if reachable.insert(unverified_idx) {
                queue.push_back(unverified_idx);
            }
        }

        // 3. Forward BFS graph traversal along outgoing dependency/call edges
        while let Some(u) = queue.pop_front() {
            if let Some(callees) = self.outgoing_edges.get(&u) {
                for &v in callees {
                    if reachable.insert(v) {
                        queue.push_back(v);
                    }
                }
            }
        }

        // 4. Identify unreachable symbols (excluding unverified symbols)
        let mut unreachable_indices: BTreeSet<usize> = BTreeSet::new();
        for (idx, _) in self.symbols.iter().enumerate() {
            if !reachable.contains(&idx) && !self.unverified_indices.contains(&idx) {
                unreachable_indices.insert(idx);
            }
        }

        // 5. Detect unreachable clusters and circular dead cycles among unreachable symbols
        let mut clusters = Vec::new();
        let mut visited_unreachable: HashSet<usize> = HashSet::new();

        for &u_idx in &unreachable_indices {
            if visited_unreachable.contains(&u_idx) {
                continue;
            }

            // Collect weakly connected component among unreachable symbols
            let mut component: Vec<usize> = Vec::new();
            let mut comp_queue = VecDeque::new();
            comp_queue.push_back(u_idx);
            visited_unreachable.insert(u_idx);

            while let Some(curr) = comp_queue.pop_front() {
                component.push(curr);

                // Explore outgoing edges within unreachable set
                if let Some(neighbors) = self.outgoing_edges.get(&curr) {
                    for &nxt in neighbors {
                        if unreachable_indices.contains(&nxt) && visited_unreachable.insert(nxt) {
                            comp_queue.push_back(nxt);
                        }
                    }
                }

                // Explore incoming edges within unreachable set
                if let Some(neighbors) = self.incoming_edges.get(&curr) {
                    for &nxt in neighbors {
                        if unreachable_indices.contains(&nxt) && visited_unreachable.insert(nxt) {
                            comp_queue.push_back(nxt);
                        }
                    }
                }
            }

            // Only group into a cluster if there are multiple symbols calling/referencing each other
            if component.len() > 1 {
                let mut internal_calls = Vec::new();
                let comp_set: HashSet<usize> = component.iter().copied().collect();

                for &node in &component {
                    if let Some(callees) = self.outgoing_edges.get(&node) {
                        for &callee in callees {
                            if comp_set.contains(&callee) {
                                internal_calls.push((
                                    self.symbols[node].key.name.clone(),
                                    self.symbols[callee].key.name.clone(),
                                ));
                            }
                        }
                    }
                }

                let cycle = detect_cycle(&component, &self.outgoing_edges, &comp_set);
                let cluster_items: Vec<DeadItem> = component
                    .iter()
                    .map(|&idx| {
                        let decl = &self.symbols[idx];
                        DeadItem {
                            name: decl.key.name.clone(),
                            kind: decl.kind.clone(),
                            file: decl.key.file.clone(),
                            line: decl.key.line,
                            col: decl.key.col,
                            exported: decl.exported,
                        }
                    })
                    .collect();

                clusters.push(UnreachableCluster {
                    symbols: cluster_items,
                    cycle,
                    internal_calls,
                });
            }
        }

        // 6. Build the list of all unreachable items
        let unreachable_items: Vec<DeadItem> = unreachable_indices
            .iter()
            .map(|&idx| {
                let decl = &self.symbols[idx];
                DeadItem {
                    name: decl.key.name.clone(),
                    kind: decl.kind.clone(),
                    file: decl.key.file.clone(),
                    line: decl.key.line,
                    col: decl.key.col,
                    exported: decl.exported,
                }
            })
            .collect();

        let reachable_keys: Vec<SymbolKey> = reachable
            .iter()
            .filter(|&&idx| !self.unverified_indices.contains(&idx))
            .map(|&idx| self.symbols[idx].key.clone())
            .collect();

        let summary = ReachabilitySummary {
            roots_count: roots.len(),
            reachable_count: reachable_keys.len(),
            unreachable_count: unreachable_items.len(),
            cluster_count: clusters.len(),
        };

        ReachabilityResult {
            summary,
            roots,
            reachable_keys,
            unreachable_items,
            unreachable_clusters: clusters,
            unverified: Vec::new(),
        }
    }
}

/// Detects if there is a directed cycle within a subset of graph nodes.
fn detect_cycle(
    nodes: &[usize],
    outgoing: &HashMap<usize, HashSet<usize>>,
    allowed: &HashSet<usize>,
) -> bool {
    let mut visited = HashSet::new();
    let mut on_stack = HashSet::new();

    fn dfs(
        curr: usize,
        outgoing: &HashMap<usize, HashSet<usize>>,
        allowed: &HashSet<usize>,
        visited: &mut HashSet<usize>,
        on_stack: &mut HashSet<usize>,
    ) -> bool {
        visited.insert(curr);
        on_stack.insert(curr);

        if let Some(callees) = outgoing.get(&curr) {
            for &next in callees {
                if !allowed.contains(&next) {
                    continue;
                }
                if !visited.contains(&next) {
                    if dfs(next, outgoing, allowed, visited, on_stack) {
                        return true;
                    }
                } else if on_stack.contains(&next) {
                    return true;
                }
            }
        }

        on_stack.remove(&curr);
        false
    }

    for &node in nodes {
        if !visited.contains(&node) && dfs(node, outgoing, allowed, &mut visited, &mut on_stack) {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_root_entry_point_classification() {
        // main and init are roots
        assert!(
            is_root_entry_point("rust", "src/main.rs", "main", "function", false, false).0
        );
        assert!(is_root_entry_point("go", "pkg/init.go", "init", "function", false, false).0);

        // Exported symbols are roots in library mode (include_exported = false)
        assert!(
            is_root_entry_point("rust", "src/lib.rs", "compute", "function", true, false).0
        );
        // Exported symbols are NOT roots when include_exported = true
        assert!(
            !is_root_entry_point("rust", "src/lib.rs", "compute", "function", true, true).0
        );

        // Trait methods are roots (dynamic dispatch)
        assert!(
            is_root_entry_point("rust", "src/lib.rs", "draw", "trait-method", false, true).0
        );

        // Test functions are roots
        assert!(
            is_root_entry_point("rust", "src/lib.rs", "test_addition", "function", false, true).0
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
        assert!(result.reachable_keys.contains(&SymbolKey::new("src/main.rs", "main", 1, 1)));
        assert!(result.reachable_keys.contains(&SymbolKey::new("src/main.rs", "active_fn", 5, 1)));

        // cycle_a and cycle_b are UNREACHABLE
        assert_eq!(result.summary.unreachable_count, 2);
        let unreachable_names: Vec<&str> =
            result.unreachable_items.iter().map(|d| d.name.as_str()).collect();
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
}
