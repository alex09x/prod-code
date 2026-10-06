/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use serde::Serialize;

#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct DeadItem {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
    /// Exported / public: nothing in this checkout uses it, but something outside might.
    pub exported: bool,
}

/// Options controlling dead-code and reachability scanning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadCodeOptions {
    pub include_exported: bool,
    pub max_files: usize,
    pub reachability: bool,
}

impl Default for DeadCodeOptions {
    fn default() -> Self {
        Self {
            include_exported: false,
            max_files: 400,
            reachability: false,
        }
    }
}

/// Candidate symbol extracted from document symbols with exact declaration range coordinates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateSymbol {
    pub name: String,
    pub kind: String,
    pub line: u32,
    pub col: u32,
    pub range_start: (u32, u32),
    pub range_end: (u32, u32),
}

#[derive(Debug, Clone, Serialize)]
pub struct DeadCodeReport {
    pub language: String,
    pub files_scanned: usize,
    pub symbols_checked: usize,
    pub dead: Vec<DeadItem>,
    /// Methods without direct references: they may still be reached through a trait,
    /// interface or protocol, which reference search does not follow.
    pub methods_unreferenced: Vec<DeadItem>,
    /// Exported symbols without references that were not listed (`include_exported` off).
    pub exported_unreferenced: usize,
    pub truncated: bool,
    /// Files and symbols the analyzer could not answer for: nothing is known about them, so
    /// none is listed as dead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unverified: Vec<Unverified>,
    /// Whole-program reachability summary metrics (when reachability analysis is enabled).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reachability: Option<crate::reachability::ReachabilitySummary>,
    /// Unreachable call clusters and circular dead cycles detected by reachability analysis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreachable_clusters: Vec<crate::reachability::UnreachableCluster>,
    /// Root entry points identified for reachability analysis.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub root_entry_points: Vec<DeadItem>,
}

/// A file whose symbols, or a symbol whose references, the analyzer did not establish.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct Unverified {
    pub file: String,
    /// The symbol; `None` when the file's symbols could not be listed at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// 1-based line and column of the symbol's name; 0 for a whole file.
    pub line: u32,
    pub col: u32,
    pub reason: String,
}

impl DeadCodeReport {
    /// Whether every source file and every symbol was judged: nothing cut by the file limit,
    /// nothing the analyzer failed to answer.
    pub fn complete(&self) -> bool {
        !self.truncated && self.unverified.is_empty()
    }

    pub fn render(&self) -> String {
        let mut out = if let Some(ref r) = self.reachability {
            format!(
                "whole-program reachability scan ({}): {} file(s), {} symbol(s) checked, {} root(s), {} reachable, {} unreachable\n",
                self.language,
                self.files_scanned,
                self.symbols_checked,
                r.roots_count,
                r.reachable_count,
                r.unreachable_count,
            )
        } else {
            format!(
                "dead code scan ({}): {} file(s), {} symbol(s) checked, {} unreferenced\n",
                self.language,
                self.files_scanned,
                self.symbols_checked,
                self.dead.len()
            )
        };
        for item in &self.dead {
            out.push_str(&format!(
                "  • {} {}{}  {}:{}:{}\n",
                item.kind,
                item.name,
                if item.exported { " (exported)" } else { "" },
                item.file,
                item.line,
                item.col
            ));
        }
        if !self.unreachable_clusters.is_empty() {
            out.push_str(&format!(
                "unreachable circular dead clusters ({}):\n",
                self.unreachable_clusters.len()
            ));
            for cluster in &self.unreachable_clusters {
                out.push_str(&format!(
                    "  • cluster ({} symbols{}, calls: {}):\n",
                    cluster.symbols.len(),
                    if cluster.cycle {
                        ", cycle detected"
                    } else {
                        ""
                    },
                    cluster
                        .internal_calls
                        .iter()
                        .map(|(from, to)| format!("{from} -> {to}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                for item in &cluster.symbols {
                    out.push_str(&format!(
                        "    - {} {}  {}:{}:{}\n",
                        item.kind, item.name, item.file, item.line, item.col
                    ));
                }
            }
        }
        if !self.methods_unreferenced.is_empty() {
            out.push_str(&format!(
                "methods without direct references ({}; may be reached through a trait / interface):\n",
                self.methods_unreferenced.len()
            ));
            for item in &self.methods_unreferenced {
                out.push_str(&format!(
                    "  • {}{}  {}:{}:{}\n",
                    item.name,
                    if item.exported { " (exported)" } else { "" },
                    item.file,
                    item.line,
                    item.col
                ));
            }
        }
        if self.exported_unreferenced > 0 {
            out.push_str(&format!(
                "{} exported symbol(s) are unreferenced inside the checkout (list them with --include-exported)\n",
                self.exported_unreferenced
            ));
        }
        if self.truncated {
            out.push_str("scan truncated by the file limit\n");
        }
        if !self.unverified.is_empty() {
            out.push_str(&format!(
                "{} could not be checked (the analyzer failed or gave no usable answer), so none is listed as dead:\n",
                self.unverified.len()
            ));
            for u in &self.unverified {
                out.push_str(&match &u.name {
                    Some(name) => format!(
                        "  • {name}  {}:{}:{}: {}\n",
                        u.file, u.line, u.col, u.reason
                    ),
                    None => format!("  • {}: {}\n", u.file, u.reason),
                });
            }
        }
        out
    }
}
