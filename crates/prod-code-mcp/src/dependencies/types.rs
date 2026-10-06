/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyNode {
    pub name: String,
    pub path: String,
    /// Number of other nodes that depend on this node (incoming).
    pub afferent_coupling: usize,
    /// Number of other nodes that this node depends on (outgoing).
    pub efferent_coupling: usize,
    /// Instability index: Ce / (Ca + Ce). 0.0 = completely stable, 1.0 = completely unstable.
    pub instability: f32,
    /// Direct dependencies of this node.
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DependencyGraphReport {
    pub scope: String,
    pub total_nodes: usize,
    pub total_edges: usize,
    pub cycles_detected: usize,
    pub cycles: Vec<Vec<String>>,
    pub nodes: Vec<DependencyNode>,
    pub isolated_nodes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DependencyScope {
    #[default]
    Crates,
    Modules,
}

pub const MAX_CYCLES_DETECTED: usize = 100;
