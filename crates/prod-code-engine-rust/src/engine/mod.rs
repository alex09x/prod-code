/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! In-memory Rust analysis engine core backed by a warm Salsa database.

use anyhow::Result;
use ra_ap_ide::{AnalysisHost, FileId};
use ra_ap_load_cargo::worktrees::Worktrees;
use ra_ap_paths::AbsPathBuf;
use ra_ap_vfs::Vfs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::proc_macro_farm::ProcMacroFarmPermit;
use crate::session_overlays::SessionOverlays;
use crate::snapshot::RustEngineSnapshot;
use crate::types::{
    AssistInfo, CallEdge, DefinitionTarget, HierarchyItem, RefactorOutcome, ReferenceTarget,
    SymbolTarget, WorkspaceSymbol,
};
use crate::vfs::normalize_vfs_path;

pub mod load;
pub mod priming;
pub mod sessions;
pub mod worktree;

/// In-memory Rust analysis engine core backed by a warm Salsa database.
pub struct RustEngine {
    pub workspace_root: PathBuf,
    pub(crate) host: AnalysisHost,
    pub(crate) vfs: Arc<std::sync::RwLock<Vfs>>,
    pub worktrees: Worktrees,
    pub(crate) worktree_attachments: HashMap<PathBuf, usize>,
    pub(crate) overlays: SessionOverlays,
    /// `main` or `validation`, for the logs.
    pub(crate) label: &'static str,
    /// Changes applied to the database since the load; each starts a new revision.
    pub(crate) changes: u64,
    /// Proc-macro worker farm allocation permit. Kept alive until this engine is dropped.
    pub(crate) _proc_macro_farm_permit: Option<ProcMacroFarmPermit>,
}

impl RustEngine {
    /// Names this engine in the logs (`main` or `validation`).
    pub fn set_label(&mut self, label: &'static str) {
        self.label = label;
    }

    /// Obtain a lightweight, thread-safe analysis snapshot for parallel execution.
    pub fn snapshot(&self) -> RustEngineSnapshot {
        RustEngineSnapshot::new(
            self.workspace_root.clone(),
            self.host.analysis(),
            Arc::clone(&self.vfs),
            self.worktrees.views(),
            None,
            self.label,
            self.changes,
        )
    }

    /// Obtain a lightweight, thread-safe analysis snapshot configured for a specific workspace or worktree view.
    pub fn snapshot_for(&self, root: &Path) -> RustEngineSnapshot {
        let abs = AbsPathBuf::assert_utf8(root.to_path_buf());
        let views = self.worktrees.views();
        let overlay = views.overlay_of(&abs).cloned();
        RustEngineSnapshot::new(
            root.to_path_buf(),
            self.host.analysis(),
            Arc::clone(&self.vfs),
            views,
            overlay,
            self.label,
            self.changes,
        )
    }

    /// Obtain an analysis snapshot routed through the worktree overlay containing `path`, if any.
    pub fn snapshot_for_path(&self, path: &Path) -> RustEngineSnapshot {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let abs = AbsPathBuf::assert_utf8(norm);
        let views = self.worktrees.views();
        let overlay = views.overlay_of(&abs).cloned();
        if let Some(ref o) = overlay {
            RustEngineSnapshot::new(
                PathBuf::from(o.worktree_root.as_str()),
                self.host.analysis(),
                Arc::clone(&self.vfs),
                views,
                overlay,
                self.label,
                self.changes,
            )
        } else {
            self.snapshot()
        }
    }

    /// Trigger cooperative Salsa cancellation on all active snapshots of this engine,
    /// recycling stuck worker queries when capacity is threatened.
    pub fn trigger_cancellation(&mut self) {
        self.host.trigger_cancellation();
    }

    pub fn workspace_symbols_for(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Vec<WorkspaceSymbol>> {
        self.snapshot_for(root).workspace_symbols(query, limit)
    }

    /// Lookup Vfs FileId for a filesystem path.
    pub fn file_id_for_path(&self, path: &Path) -> Option<FileId> {
        self.snapshot_for_path(path).file_id_for_path(path)
    }

    /// Lookup filesystem path for a Vfs FileId.
    pub fn path_for_file_id(&self, file_id: FileId) -> Option<PathBuf> {
        self.snapshot().path_for_file_id(file_id)
    }

    /// Retrieve symbol type, docs, and signature at (line, col).
    pub fn hover(&self, path: &Path, line: u32, col: u32) -> Result<Option<String>> {
        self.snapshot_for_path(path).hover(path, line, col)
    }

    /// Jump to symbol definition from (line, col).
    pub fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        self.snapshot_for_path(path)
            .goto_definition(path, line, col)
    }

    /// Find all references to symbol at (line, col) across entire workspace.
    pub fn find_all_refs(&self, path: &Path, line: u32, col: u32) -> Result<Vec<ReferenceTarget>> {
        self.snapshot_for_path(path).find_all_refs(path, line, col)
    }

    pub fn prepare_call_hierarchy(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<HierarchyItem>> {
        self.snapshot_for_path(path)
            .prepare_call_hierarchy(path, line, col)
    }

    pub fn incoming_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        self.snapshot_for_path(path).incoming_calls(path, line, col)
    }

    pub fn outgoing_calls(&self, path: &Path, line: u32, col: u32) -> Result<Vec<CallEdge>> {
        self.snapshot_for_path(path).outgoing_calls(path, line, col)
    }

    pub fn goto_implementation(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<DefinitionTarget>> {
        self.snapshot_for_path(path)
            .goto_implementation(path, line, col)
    }

    /// Code actions at a position; see [`RustEngineSnapshot::list_assists`].
    pub fn list_assists(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
    ) -> Result<Vec<AssistInfo>> {
        self.snapshot_for_path(path)
            .list_assists(path, line, col, end)
    }

    /// Apply one code action; see [`RustEngineSnapshot::apply_assist`].
    pub fn apply_assist(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        end: Option<(u32, u32)>,
        id: &str,
        subtype: Option<usize>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot_for_path(path)
            .apply_assist(path, line, col, end, id, subtype)
    }

    /// Delete an unreferenced item; see [`RustEngineSnapshot::safe_delete`].
    pub fn safe_delete(
        &self,
        path: &Path,
        line: u32,
        col: u32,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot_for_path(path).safe_delete(path, line, col)
    }

    /// Rename the symbol at (line, col); see [`RustEngineSnapshot::rename`].
    pub fn rename(
        &self,
        path: &Path,
        line: u32,
        col: u32,
        new_name: &str,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot_for_path(path)
            .rename(path, line, col, new_name)
    }

    pub fn structural_replace(
        &self,
        rule: &str,
        context: &Path,
        line: u32,
        col: u32,
        scope: Option<&Path>,
    ) -> Result<std::result::Result<RefactorOutcome, String>> {
        self.snapshot_for_path(context)
            .structural_replace(rule, context, line, col, scope)
    }

    /// Generate outline / document symbols for a file.
    pub fn document_symbols(&self, path: &Path) -> Result<Vec<SymbolTarget>> {
        self.snapshot_for_path(path).document_symbols(path)
    }

    /// Workspace-wide symbol search by name; see the snapshot method.
    pub fn workspace_symbols(&self, query: &str, limit: usize) -> Result<Vec<WorkspaceSymbol>> {
        self.snapshot().workspace_symbols(query, limit)
    }
}
