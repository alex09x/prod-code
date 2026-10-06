/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{FileEntry, Indexed};
use std::collections::{HashMap, HashSet};

/// Typed AST dependency graph across workspace declarations.
#[derive(Default, Clone, Debug)]
pub struct TypedGraph {
    /// In-degree centrality: how many other declarations in the workspace mention this declaration's
    /// identifier in their signatures, containers, or doc comments.
    pub in_degree: HashMap<String, usize>,
    /// Adjacency: for each declaration identifier, which other known declaration identifiers it mentions.
    pub mentions: HashMap<String, HashSet<String>>,
    /// Reverse adjacency: which declaration identifiers mention this declaration identifier.
    pub referenced_by: HashMap<String, HashSet<String>>,
}

/// Syntactic category weight for declaration kinds.
pub fn category_weight(kind: &str) -> f64 {
    match kind {
        "struct" | "class" | "interface" | "trait" | "protocol" => 1.35,
        "function" | "method" | "macro" => 1.20,
        "impl" | "extension" => 1.10,
        "enum" | "type" => 1.05,
        "module" => 1.00,
        "constant" | "variable" => 0.85,
        _ => 1.00,
    }
}

/// Structural centrality score factoring category weight and in-degree.
pub fn centrality_score(kind: &str, in_degree: usize) -> f64 {
    let cat = category_weight(kind);
    cat * (1.0 + 0.35 * (in_degree as f64).ln_1p())
}

#[derive(Default)]
pub struct WorkspaceIndex {
    pub(crate) files: HashMap<String, FileEntry>,
    /// Set until the first full walk; afterwards the index is kept current by the sync layer
    /// telling us which files it wrote, so a query never walks the tree.
    pub(crate) built: bool,
    /// Files the sync layer touched since the last query, to be reindexed on the next one.
    pub(crate) pending: Vec<String>,
    /// Pre-indexed typed AST graph across workspace declarations.
    pub graph: TypedGraph,
}

impl WorkspaceIndex {
    pub(crate) fn declarations(&self) -> impl Iterator<Item = &Indexed> {
        self.files.values().flat_map(|f| f.decls.iter())
    }

    pub fn len(&self) -> usize {
        self.files.values().map(|f| f.decls.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Builds the typed AST dependency graph across all currently indexed declarations.
    pub fn build_graph(&self) -> TypedGraph {
        let mut in_degree: HashMap<String, usize> = HashMap::new();
        let mut mentions: HashMap<String, HashSet<String>> = HashMap::new();
        let mut referenced_by: HashMap<String, HashSet<String>> = HashMap::new();

        let mut known_names: HashSet<&str> = HashSet::new();
        for decl in self.declarations() {
            if !decl.decl.name.is_empty() {
                known_names.insert(&decl.decl.name);
            }
        }

        for d in self.declarations() {
            let d_name = &d.decl.name;
            let mut seen_in_d: HashSet<String> = HashSet::new();
            let mut scan_text = |text: &str| {
                for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
                    if !word.is_empty() && word != d_name.as_str() && known_names.contains(word) {
                        seen_in_d.insert(word.to_string());
                    }
                }
            };
            scan_text(&d.decl.signature);
            if let Some(c) = &d.decl.container {
                scan_text(c);
            }
            scan_text(&d.decl.doc);

            for target in seen_in_d {
                *in_degree.entry(target.clone()).or_insert(0) += 1;
                mentions
                    .entry(d_name.clone())
                    .or_default()
                    .insert(target.clone());
                referenced_by
                    .entry(target)
                    .or_default()
                    .insert(d_name.clone());
            }
        }

        TypedGraph {
            in_degree,
            mentions,
            referenced_by,
        }
    }

    /// Recomputes and updates the cached typed graph.
    pub fn rebuild_graph(&mut self) {
        self.graph = self.build_graph();
    }
}
