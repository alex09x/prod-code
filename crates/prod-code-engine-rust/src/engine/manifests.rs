/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Manifest tracking, freshness validation, and live reload of dependency graphs.

use anyhow::Result;
use ra_ap_load_cargo::worktree::Overlay;
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{ProjectManifest, ProjectWorkspace, ProjectWorkspaceKind};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::RustEngine;
use super::load::{extensionless_target_roots, load_target_roots, path_attribute_includes};
use crate::config::ProdCodeConfig;
use crate::vfs::normalize_vfs_path;

impl RustEngine {
    /// Returns `true` when any local Cargo package under `root` (`self.workspace_root` or an
    /// attached worktree copy) has a standard auto-discovered target root on disk (`tests/*.rs`,
    /// `tests/*/main.rs`, `examples/*.rs`, `benches/*.rs`, `src/bin/*.rs`) that is not yet
    /// indexed as a crate target in `root`'s analyzer view (#1777).
    fn has_unregistered_auto_targets_at(&self, root: &Path) -> bool {
        let base = self.worktrees.base();
        let ProjectWorkspaceKind::Cargo { cargo, .. } = &base.kind else {
            return false;
        };
        let snap = self.snapshot_for(root);
        for pkg in cargo.packages() {
            if !cargo[pkg].is_local {
                continue;
            }
            let Some(base_pkg_dir) = Path::new(cargo[pkg].manifest.as_str()).parent() else {
                continue;
            };
            let pkg_dir = if root == self.workspace_root {
                base_pkg_dir.to_path_buf()
            } else if let Ok(rel) = base_pkg_dir.strip_prefix(&self.workspace_root) {
                root.join(rel)
            } else {
                continue;
            };
            for rel_dir in ["tests", "examples", "benches", "src/bin"] {
                let dir = pkg_dir.join(rel_dir);
                let Ok(entries) = std::fs::read_dir(&dir) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) == Some("rs") && path.is_file() {
                        if snap.file_id_for_path(&path).is_none() {
                            return true;
                        }
                    } else if path.is_dir() {
                        let main_rs = path.join("main.rs");
                        if main_rs.is_file() && snap.file_id_for_path(&main_rs).is_none() {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
    /// Collect all manifest and lockfile paths that determine the base workspace crate graph.
    pub(crate) fn collect_base_manifest_paths(&self) -> Vec<PathBuf> {
        let base = self.worktrees.base();
        let mut paths = Vec::new();
        let ws_root = base.workspace_root();
        paths.push(PathBuf::from(ws_root.join("Cargo.lock").as_str()));
        paths.push(PathBuf::from(ws_root.join("Cargo.toml").as_str()));
        if let ProjectWorkspaceKind::Cargo { cargo, .. } = &base.kind {
            paths.push(PathBuf::from(cargo.manifest_path().as_str()));
            for pkg in cargo.packages() {
                if cargo[pkg].is_local {
                    paths.push(PathBuf::from(cargo[pkg].manifest.as_str()));
                }
            }
        }
        for cfg in [
            ".cargo/config.toml",
            ".cargo/config",
            "rust-toolchain.toml",
            "rust-toolchain",
        ] {
            paths.push(PathBuf::from(ws_root.join(cfg).as_str()));
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Collect manifest paths for a worktree copy root.
    pub(crate) fn collect_worktree_manifest_paths(
        &self,
        copy_root: &Path,
        workspace: Option<&ProjectWorkspace>,
    ) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        paths.push(copy_root.join("Cargo.lock"));
        paths.push(copy_root.join("Cargo.toml"));
        if let Some(ws) = workspace {
            if let ProjectWorkspaceKind::Cargo { cargo, .. } = &ws.kind {
                paths.push(PathBuf::from(cargo.manifest_path().as_str()));
                for pkg in cargo.packages() {
                    if cargo[pkg].is_local {
                        paths.push(PathBuf::from(cargo[pkg].manifest.as_str()));
                    }
                }
            }
        } else {
            let base_paths = self.collect_base_manifest_paths();
            for bp in base_paths {
                if let Ok(rel) = bp.strip_prefix(&self.workspace_root) {
                    paths.push(copy_root.join(rel));
                }
            }
        }
        for cfg in [
            ".cargo/config.toml",
            ".cargo/config",
            "rust-toolchain.toml",
            "rust-toolchain",
        ] {
            paths.push(copy_root.join(cfg));
        }
        paths.sort();
        paths.dedup();
        paths
    }

    /// Record modification times for base workspace manifests.
    pub(crate) fn record_base_manifests(&mut self) {
        let paths = self.collect_base_manifest_paths();
        self.base_manifest_mtimes.clear();
        for p in paths {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            self.base_manifest_mtimes.insert(p, mtime);
        }
    }

    /// Record modification times for a worktree's manifests.
    pub(crate) fn record_worktree_manifests(
        &mut self,
        copy_root: &Path,
        workspace: Option<&ProjectWorkspace>,
    ) {
        let paths = self.collect_worktree_manifest_paths(copy_root, workspace);
        let mut map = HashMap::new();
        for p in paths {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            map.insert(p, mtime);
        }
        self.worktree_manifest_mtimes
            .insert(copy_root.to_path_buf(), map);
    }

    /// Check if base workspace manifests or lockfiles have changed on disk.
    pub fn is_base_stale(&self) -> bool {
        if self.base_manifest_mtimes.is_empty() {
            return false;
        }
        for (path, recorded_mtime) in &self.base_manifest_mtimes {
            let current = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if current != *recorded_mtime {
                return true;
            }
        }
        self.has_unregistered_auto_targets_at(&self.workspace_root)
    }

    /// Check if a worktree's manifests or lockfiles have changed on disk.
    pub fn is_worktree_stale(&self, copy_root: &Path) -> bool {
        if copy_root == self.workspace_root || !self.has_worktree(copy_root) {
            return false;
        }
        if self.is_base_stale() {
            return true;
        }
        let Some(recorded) = self.worktree_manifest_mtimes.get(copy_root) else {
            return true;
        };
        for (path, recorded_mtime) in recorded {
            let current = std::fs::metadata(path).and_then(|m| m.modified()).ok();
            if current != *recorded_mtime {
                return true;
            }
        }
        self.has_unregistered_auto_targets_at(copy_root)
    }

    /// Reload the base workspace when its manifests or dependencies have changed.
    pub fn reload_base(&mut self) -> Result<()> {
        let abs_root = AbsPathBuf::assert_utf8(self.workspace_root.clone());
        let manifest = ProjectManifest::discover_single(&abs_root)
            .map_err(|e| anyhow::anyhow!("Manifest discovery failed: {e}"))?;
        let config = ProdCodeConfig::load(&self.workspace_root);
        let cargo_config = config.cargo_config_for(&self.workspace_root);
        let mut ws = ProjectWorkspace::load(manifest, &cargo_config, &|_| {})
            .map_err(|e| anyhow::anyhow!("Project workspace load failed: {e}"))?;
        if config.rust.build_scripts
            && let Ok(scripts) = ws.run_build_scripts(&cargo_config, &|_| {})
        {
            ws.set_build_scripts(scripts);
        }
        let target_roots = extensionless_target_roots(&ws);
        {
            self.prepare_mutation();
            let db = self.host.raw_database_mut();
            let mut vfs = self
                .vfs
                .write()
                .map_err(|e| anyhow::anyhow!("VFS lock: {e}"))?;
            ws.extra_includes
                .extend(path_attribute_includes(&vfs, ws.workspace_root()));
            ws.extra_includes.sort();
            ws.extra_includes.dedup();
            load_target_roots(&mut self.worktrees, db, &mut vfs, &target_roots);
            self.worktrees.set_base(db, &mut vfs, ws);
        }
        self.changes += 1;
        self.record_base_manifests();
        tracing::info!(root = %self.workspace_root.display(), "Reloaded base workspace in shared RustEngine");
        Ok(())
    }

    /// Reload a worktree overlay when its manifests or dependencies have changed.
    pub fn reload_worktree(&mut self, copy_root: &Path) -> Result<()> {
        let copy_abs = AbsPathBuf::assert_utf8(copy_root.to_path_buf());
        let base_abs = AbsPathBuf::assert_utf8(self.workspace_root.clone());
        let overlay = Overlay {
            worktree_root: copy_abs.clone(),
            base_root: base_abs,
        };
        let config = ProdCodeConfig::load(copy_root);
        let cargo_config = config.cargo_config_for(copy_root);
        let mut workspace = match self.worktrees.workspace_of_copy(&overlay) {
            Some(ws) => ws,
            None => {
                let manifest = ProjectManifest::discover_single(&copy_abs).map_err(|e| {
                    anyhow::anyhow!(
                        "Manifest discovery failed for copy {}: {e}",
                        copy_root.display()
                    )
                })?;
                ProjectWorkspace::load(manifest, &cargo_config, &|_| {}).map_err(|e| {
                    anyhow::anyhow!(
                        "Workspace load failed for copy {}: {e}",
                        copy_root.display()
                    )
                })?
            }
        };
        let target_roots = extensionless_target_roots(&workspace);
        self.changes += 1;
        self.record_worktree_manifests(copy_root, Some(&workspace));
        {
            self.prepare_mutation();
            let db = self.host.raw_database_mut();
            let mut vfs = self
                .vfs
                .write()
                .map_err(|e| anyhow::anyhow!("VFS lock: {e}"))?;
            let base_root = self.worktrees.base().workspace_root().to_path_buf();
            let worktree_root = workspace.workspace_root().to_path_buf();
            for include in &self.worktrees.base().extra_includes {
                let Ok(relative) =
                    Path::new(include.as_str()).strip_prefix(Path::new(base_root.as_path()))
                else {
                    continue;
                };
                workspace.extra_includes.push(
                    AbsPathBuf::assert_utf8(Path::new(worktree_root.as_str()).join(relative))
                        .normalize(),
                );
            }
            workspace
                .extra_includes
                .extend(path_attribute_includes(&vfs, worktree_root.as_path()));
            workspace.extra_includes.sort();
            workspace.extra_includes.dedup();
            load_target_roots(&mut self.worktrees, db, &mut vfs, &target_roots);
            if config.rust.build_scripts {
                self.worktrees
                    .inherit_build_scripts(&vfs, &mut workspace, &overlay);
            }
            self.worktrees.add(db, &mut vfs, workspace, overlay);
        }
        tracing::info!(copy = %copy_root.display(), "Reloaded worktree overlay in shared RustEngine");
        Ok(())
    }

    /// Reload the worktree or base workspace if their manifests are stale.
    pub fn ensure_fresh_for_path(&mut self, path: &Path) -> Result<bool> {
        let mut reloaded = false;
        if self.is_base_stale() {
            self.reload_base()?;
            reloaded = true;
        }

        let norm = normalize_vfs_path(path, &self.workspace_root);
        let abs = AbsPathBuf::assert_utf8(norm);
        let target_worktree = {
            let views = self.worktrees.views();
            views
                .overlay_of(&abs)
                .map(|o| PathBuf::from(o.worktree_root.as_str()))
        };
        if let Some(worktree_root) = target_worktree {
            if self.is_worktree_stale(&worktree_root) {
                self.reload_worktree(&worktree_root)?;
                return Ok(true);
            }
            return Ok(reloaded);
        }
        let matching_copy = self
            .worktree_attachments
            .keys()
            .find(|copy_root| path.starts_with(copy_root))
            .cloned();
        if let Some(copy_root) = matching_copy {
            if self.is_worktree_stale(&copy_root) {
                self.reload_worktree(&copy_root)?;
                return Ok(true);
            }
            return Ok(reloaded);
        }
        Ok(reloaded)
    }
}

#[cfg(test)]
mod tests;
