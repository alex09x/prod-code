/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! DTOs and outcome types for queries and refactorings.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DefinitionTarget {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub name: String,
}

/// A function, method or other item in the call hierarchy: where it is declared (`line`/`col`
/// point at its name, `end_line`/`end_col` close the whole item) and what kind of item it is.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HierarchyItem {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
}

/// One edge of the call graph: the caller or callee item and the 1-based positions of the
/// call sites (in the caller's file for incoming calls, in this item's file for outgoing).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallEdge {
    pub item: HierarchyItem,
    pub call_sites: Vec<(u32, u32)>,
    /// The caller is a test (`#[test]` or inside a `cfg(test)` module), as rust-analyzer
    /// classifies it.
    #[serde(default)]
    pub is_test: bool,
}

/// One rust-analyzer diagnostic for a file, computed in memory (no cargo check).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileDiagnostic {
    pub code: String,
    pub message: String,
    /// `error`, `warning`, `weak` (hint) or `allow`.
    pub severity: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub unused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReferenceTarget {
    pub path: PathBuf,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymbolTarget {
    pub name: String,
    pub kind: String,
    /// 1-based line of the item's name.
    pub line: u32,
    /// 1-based column of the item's name.
    #[serde(default)]
    pub col: u32,
    /// 1-based last line of the whole item (its body included).
    #[serde(default)]
    pub end_line: u32,
    pub detail: Option<String>,
    /// Labels of the enclosing items, outermost first (`mod tests`, `impl Shape for Circle`).
    #[serde(default)]
    pub containers: Vec<String>,
}

/// A file rewritten by a refactoring: its full new content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RewrittenFile {
    pub path: PathBuf,
    pub new_text: String,
    /// Number of individual text edits folded into `new_text`.
    pub edits: usize,
    /// Line count of the previous content (for whole-file replacement ranges).
    pub old_line_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// Everything a refactoring changes: rewritten files, new files, and moves/renames of files
/// or directories (a module rename renames its file).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefactorOutcome {
    pub files: Vec<RewrittenFile>,
    pub created: Vec<RewrittenFile>,
    pub moves: Vec<FileMove>,
}

impl RefactorOutcome {
    pub fn total_edits(&self) -> usize {
        self.files.iter().map(|f| f.edits).sum()
    }
}

/// A code action rust-analyzer offers at a position or selection (inline, extract, generate,
/// rewrite, quick fix). `id` plus `subtype` identify it for `RustEngineSnapshot::apply_assist`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssistInfo {
    pub id: String,
    pub kind: String,
    pub subtype: Option<usize>,
    pub label: String,
    pub group: Option<String>,
}

/// One hit of a workspace-wide symbol search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSymbol {
    pub path: PathBuf,
    pub name: String,
    pub kind: String,
    /// 1-based line and column of the name.
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    /// Enclosing item (`impl` self type, module, trait), when the index knows it. For a hit in
    /// a dependency crate it is the module path, starting with that crate
    /// (`tokio_util::codec::framed`).
    pub container: Option<String>,
}
