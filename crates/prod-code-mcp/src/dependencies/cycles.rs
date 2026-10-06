/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::PathBuf;

use super::types::MAX_CYCLES_DETECTED;

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
