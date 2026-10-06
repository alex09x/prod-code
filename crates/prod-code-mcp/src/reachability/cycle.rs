/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{HashMap, HashSet};

/// Detects if there is a directed cycle within a subset of graph nodes.
pub fn detect_cycle(
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
