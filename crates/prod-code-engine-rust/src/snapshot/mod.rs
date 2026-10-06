/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Thread-safe Salsa database analysis snapshot for concurrent queries.

use anyhow::{Context, Result};
use ra_ap_ide::{
    Analysis, AssistConfig, DiagnosticsConfig, FileId, FilePosition, NavigationTarget,
};
use ra_ap_ide_db::SnippetCap;
use ra_ap_load_cargo::worktree::Overlay;
use ra_ap_load_cargo::worktrees::Views;
use ra_ap_paths::AbsPathBuf;
use ra_ap_vfs::{AnchoredPathBuf, Vfs};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::types::HierarchyItem;
use crate::vfs::{is_safe_file_id, line_col_to_offset, normalize_vfs_path, offset_to_line_col};

pub mod diagnostics;
pub mod hierarchy;
pub mod navigation;
pub mod refactor;
pub mod ssr;

/// Thread-safe, multi-core analysis snapshot backed by warm Salsa database.
///
/// Snapshots can be moved across threads (`tokio::task::spawn_blocking`),
/// allowing concurrent read queries (hover, definition, references, symbols)
/// to execute simultaneously across all available CPU cores without blocking.
pub struct RustEngineSnapshot {
    pub workspace_root: PathBuf,
    pub(crate) analysis: Analysis,
    pub(crate) vfs: Arc<std::sync::RwLock<Vfs>>,
    pub views: Views,
    pub overlay: Option<Overlay>,
    /// Which engine of the workspace this is a snapshot of, and how many changes that engine
    /// had applied when it was taken: a diagnostics pass is logged with both, so a cold pass
    /// can be traced to the write that made it cold (#235).
    pub(crate) label: &'static str,
    pub(crate) changes: u64,
}

impl RustEngineSnapshot {
    pub(crate) fn new(
        workspace_root: PathBuf,
        analysis: Analysis,
        vfs: Arc<std::sync::RwLock<Vfs>>,
        views: Views,
        overlay: Option<Overlay>,
        label: &'static str,
        changes: u64,
    ) -> Self {
        Self {
            workspace_root,
            analysis,
            vfs,
            views,
            overlay,
            label,
            changes,
        }
    }

    /// Lookup Vfs FileId for a filesystem path with safe 24-bit EditionedFileId validation.
    pub fn file_id_for_path(&self, path: &Path) -> Option<FileId> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let abs = AbsPathBuf::assert_utf8(norm);
        let guard = self.vfs.read().ok()?;
        let file_id = self.views.file(&guard, &abs)?;
        let file_id = self.views.analyzed_file(&guard, file_id, |f| {
            self.analysis.crates_for(f).is_ok_and(|c| !c.is_empty())
        });
        if is_safe_file_id(file_id) {
            let overlay = self.current_overlay();
            if !self.is_in_view(file_id, overlay.as_ref()) {
                return None;
            }
            Some(file_id)
        } else {
            tracing::error!(?file_id, "FileId exceeded MAX_SAFE_FILE_ID (0x007F_FFFF)");
            None
        }
    }

    /// Lookup filesystem path for a Vfs FileId, mapped to this snapshot's workspace view.
    pub fn path_for_file_id(&self, file_id: FileId) -> Option<PathBuf> {
        let overlay = self.current_overlay();
        self.path_for_file_id_in_view(file_id, overlay.as_ref())
    }

    /// The active overlay of this snapshot, from explicit configuration or workspace root.
    pub fn current_overlay(&self) -> Option<Overlay> {
        self.overlay
            .clone()
            .or_else(|| self.overlay_for_path(&self.workspace_root))
    }

    /// Lookup filesystem path for a Vfs FileId, translated to the worktree overlay view if present.
    pub fn path_for_file_id_in_view(
        &self,
        file_id: FileId,
        overlay: Option<&Overlay>,
    ) -> Option<PathBuf> {
        let guard = self.vfs.read().ok()?;
        let vfs_path = self.views.path_in_view(&guard, overlay, file_id);
        vfs_path.as_path().map(|p| PathBuf::from(p.as_str()))
    }

    /// Check whether a file is in view for the requesting workspace/overlay.
    pub fn is_in_view(&self, file_id: FileId, overlay: Option<&Overlay>) -> bool {
        let Ok(guard) = self.vfs.read() else {
            return false;
        };
        self.views.in_view(&guard, overlay, file_id, |f| {
            self.analysis.crates_for(f).is_ok_and(|c| !c.is_empty())
        })
    }

    /// Resolve the active overlay for a file or directory path.
    pub fn overlay_for_path(&self, path: &Path) -> Option<Overlay> {
        if let Some(ref o) = self.overlay {
            let norm = normalize_vfs_path(path, &self.workspace_root);
            let abs = AbsPathBuf::assert_utf8(norm);
            if abs.starts_with(&o.worktree_root) {
                return Some(o.clone());
            }
        }
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let abs = AbsPathBuf::assert_utf8(norm);
        self.views.overlay_of(&abs).cloned()
    }

    pub(crate) fn anchored_path(&self, anchored: &AnchoredPathBuf) -> Option<PathBuf> {
        let anchor = self.path_for_file_id(anchored.anchor)?;
        Some(anchor.parent()?.join(&anchored.path))
    }

    pub(crate) fn file_position(&self, path: &Path, line: u32, col: u32) -> Result<FilePosition> {
        let file_id = self
            .file_id_for_path(path)
            .with_context(|| format!("File not found in VFS: {:?}", path))?;
        let text = self.analysis.file_text(file_id)?;
        let offset = line_col_to_offset(&text, line, col)
            .with_context(|| format!("Invalid position {line}:{col} in {}", path.display()))?;
        Ok(FilePosition { file_id, offset })
    }

    pub(crate) fn hierarchy_item_in_view(
        &self,
        target: &NavigationTarget,
        overlay: Option<&Overlay>,
    ) -> Option<HierarchyItem> {
        let path = self.path_for_file_id_in_view(target.file_id, overlay)?;
        let text = self.analysis.file_text(target.file_id).ok()?;
        let focus = target.focus_range.unwrap_or(target.full_range);
        let (line, col) = offset_to_line_col(&text, focus.start());
        let (end_line, end_col) = offset_to_line_col(&text, target.full_range.end());
        Some(HierarchyItem {
            name: target.name.to_string(),
            kind: target
                .kind
                .map(|k| format!("{k:?}"))
                .unwrap_or_else(|| "Function".to_string()),
            path,
            line,
            col,
            end_line,
            end_col,
        })
    }

    pub(crate) fn assist_configs() -> (AssistConfig, DiagnosticsConfig) {
        let diagnostics = DiagnosticsConfig::test_sample();
        let assists = AssistConfig {
            snippet_cap: SnippetCap::new(false),
            allowed: None,
            insert_use: diagnostics.insert_use,
            prefer_no_std: false,
            prefer_prelude: true,
            prefer_absolute: false,
            assist_emit_must_use: false,
            term_search_fuel: 1800,
            code_action_grouping: true,
            expr_fill_default: Default::default(),
            prefer_self_ty: false,
            show_rename_conflicts: true,
        };
        (assists, diagnostics)
    }
}
