/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::dead_code::{DeadItem, Unverified};
use serde::{Deserialize, Serialize};

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
