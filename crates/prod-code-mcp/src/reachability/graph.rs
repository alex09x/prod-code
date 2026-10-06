/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};

use crate::dead_code::DeadItem;

use super::cycle::detect_cycle;
use super::types::{
    ReachabilityResult, ReachabilitySummary, SymbolDecl, SymbolKey, UnreachableCluster,
};

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
