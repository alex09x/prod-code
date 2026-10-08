/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Worktree attachments, file reloading, and Salsa database text mutation.

use anyhow::Result;
use ra_ap_paths::AbsPathBuf;
use std::path::Path;

use super::RustEngine;
use crate::vfs::{is_rust_source, normalize_vfs_path};

impl RustEngine {
    /// Text the database currently holds for `norm`, or `None` when the file is unknown.
    pub(crate) fn current_db_text(&self, norm: &Path) -> Option<String> {
        let file_id = self.file_id_for_path(norm)?;
        self.host
            .analysis()
            .file_text(file_id)
            .ok()
            .map(|text| text.to_string())
    }

    /// Writes `text` for `norm` into the database; `None` empties the file (rust-analyzer's
    /// representation of a deleted file) without touching the VFS registration.
    pub(crate) fn apply_text(&mut self, norm: &Path, text: Option<String>) -> Result<()> {
        match text {
            Some(text) => self.apply_file_change(norm, text),
            None => {
                let abs = AbsPathBuf::assert_utf8(norm.to_path_buf());
                let db = self.host.raw_database_mut();
                let mut vfs = self
                    .vfs
                    .write()
                    .map_err(|e| anyhow::anyhow!("VFS lock error: {e}"))?;
                let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    self.worktrees.set_file_text(db, &mut vfs, &abs, None);
                }));
                if let Err(panic) = res {
                    let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    anyhow::bail!(
                        "Salsa/VFS panic clearing file text for {}: {msg}",
                        norm.display()
                    );
                }
                self.changes += 1;
                Ok(())
            }
        }
    }

    pub(crate) fn restore_base(&mut self, norm: &Path) -> Result<()> {
        let base = self.overlays.base.get(norm).cloned().flatten();
        self.apply_text(norm, base)?;
        self.overlays.owner.remove(norm);
        let still_referenced = self
            .overlays
            .sessions
            .values()
            .any(|files| files.contains_key(norm));
        if !still_referenced {
            self.overlays.base.remove(norm);
        }
        Ok(())
    }

    /// Attaches a worktree copy of this workspace. Reference-counted so multiple workspaces
    /// or validation sessions sharing the same worktree root keep it attached until all release it.
    pub fn attach_worktree(&mut self, copy_root: &Path) -> Result<()> {
        let is_attached = self.worktree_attachments.contains_key(copy_root);
        let stale = is_attached && self.is_worktree_stale(copy_root);

        if is_attached && !stale {
            if let Some(count) = self.worktree_attachments.get_mut(copy_root) {
                *count += 1;
            }
            return Ok(());
        }

        self.reload_worktree(copy_root)?;

        if let Some(count) = self.worktree_attachments.get_mut(copy_root) {
            *count += 1;
        } else {
            self.worktree_attachments.insert(copy_root.to_path_buf(), 1);
        }
        tracing::info!(copy = %copy_root.display(), stale, "Attached worktree overlay to shared RustEngine");
        Ok(())
    }

    /// Detaches a worktree copy of this workspace when its attachment reference count reaches zero.
    pub fn detach_worktree(&mut self, copy_root: &Path) -> bool {
        if let Some(count) = self.worktree_attachments.get_mut(copy_root) {
            *count = count.saturating_sub(1);
            if *count > 0 {
                return false;
            }
            self.worktree_attachments.remove(copy_root);
            self.worktree_manifest_mtimes.remove(copy_root);
        }
        let copy_abs = AbsPathBuf::assert_utf8(copy_root.to_path_buf());
        let db = self.host.raw_database_mut();
        let Ok(mut vfs) = self.vfs.write() else {
            return false;
        };
        let removed = self.worktrees.remove(db, &mut vfs, &copy_abs);
        if removed {
            self.changes += 1;
            tracing::info!(copy = %copy_root.display(), "Detached worktree overlay from shared RustEngine");
        }
        removed
    }

    pub fn has_worktree(&self, copy_root: &Path) -> bool {
        self.worktree_attachments
            .get(copy_root)
            .copied()
            .unwrap_or(0)
            > 0
            || {
                let copy_abs = AbsPathBuf::assert_utf8(copy_root.to_path_buf());
                self.worktrees
                    .overlays()
                    .any(|o| o.worktree_root == copy_abs)
            }
    }

    pub fn worktree_attachment_count(&self, copy_root: &Path) -> usize {
        self.worktree_attachments
            .get(copy_root)
            .copied()
            .unwrap_or(0)
    }

    pub fn reload_file(&mut self, path: &Path) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let abs = AbsPathBuf::assert_utf8(norm);
        let db = self.host.raw_database_mut();
        let mut vfs = self
            .vfs
            .write()
            .map_err(|e| anyhow::anyhow!("VFS lock error: {e}"))?;
        self.worktrees.reload_file(db, &mut vfs, &abs);
        self.changes += 1;
        Ok(())
    }

    /// Single-owner fast path: Apply live buffer edits directly into Salsa DB in memory.
    pub fn apply_file_change(&mut self, path: &Path, new_text: String) -> Result<()> {
        if !is_rust_source(path) && self.file_id_for_path(path).is_none() {
            return Ok(());
        }
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let abs = AbsPathBuf::assert_utf8(norm);
        let db = self.host.raw_database_mut();
        let mut vfs = self
            .vfs
            .write()
            .map_err(|e| anyhow::anyhow!("VFS lock error: {e}"))?;
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.worktrees
                .set_file_text(db, &mut vfs, &abs, Some(new_text));
        }));
        if let Err(panic) = res {
            let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            anyhow::bail!(
                "Salsa/VFS panic setting file text for {}: {msg}",
                path.display()
            );
        }
        self.changes += 1;
        Ok(())
    }
}
