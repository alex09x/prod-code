/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{layout::layout_only, platform::platform_dependent};
use crate::sync::{apply_pulled_files_for, scan_workspace_files};
use anyhow::Result;
use prod_code_protocol::FileDelta;
use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::Path,
    time::SystemTime,
};

#[derive(PartialEq)]
struct FileState {
    content: Vec<u8>,
    modified: SystemTime,
    symlink: bool,
}

impl FileState {
    fn read(path: &Path) -> std::io::Result<Option<Self>> {
        Self::read_limited(path, crate::sync::MAX_NON_GIT_WORKSPACE_BYTES)
    }

    fn read_limited(path: &Path, limit: usize) -> std::io::Result<Option<Self>> {
        let meta = match std::fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let target_is_file = if meta.file_type().is_symlink() {
            std::fs::metadata(path)?.is_file()
        } else {
            meta.is_file()
        };
        if !target_is_file {
            return Err(std::io::Error::other(
                "exec writeback target is not a regular file",
            ));
        }
        // Check before allocation and cap the read too, in case the file grows meanwhile.
        if meta.len() > limit as u64 {
            return Err(std::io::Error::other(
                "exec writeback snapshot exceeds byte limit",
            ));
        }
        let mut content = Vec::new();
        std::fs::File::open(path)?
            .take(limit as u64 + 1)
            .read_to_end(&mut content)?;
        if content.len() > limit {
            return Err(std::io::Error::other(
                "exec writeback snapshot exceeds byte limit",
            ));
        }
        Ok(Some(Self {
            content,
            modified: meta.modified()?,
            symlink: meta.file_type().is_symlink(),
        }))
    }
}

/// The initial eligible writeback set, not just mtimes observed after an exec. Absence is a
/// state too: a locally removed/renamed source must differ from its preflight identity.
pub(super) struct Writeback {
    expected: HashMap<String, FileState>,
    unknown: HashSet<String>,
    unscanned_outputs: bool,
    started: SystemTime,
    formatter: bool,
    pub pulled: Vec<String>,
    pub relaid: Vec<String>,
    pub kept: Vec<String>,
    pub platform_files: Vec<String>,
}

impl Writeback {
    pub fn capture(root: &Path, enabled: bool, formatter: bool) -> Result<Self> {
        let started = SystemTime::now();
        let mut expected = HashMap::new();
        let mut unknown = HashSet::new();
        let mut unscanned_outputs = false;
        if enabled {
            // Reuse the established source boundary; transport and watermark stay unchanged.
            for delta in scan_workspace_files(root, None)? {
                let mut state =
                    FileState::read(&root.join(&delta.relative_path))?.unwrap_or_else(|| {
                        FileState {
                            content: vec![],
                            modified: started,
                            symlink: false,
                        }
                    });
                // Keep the exact bytes that established the initial identity, even if an
                // editor changed them between the source scan and the metadata read.
                state.content = delta.content.unwrap_or_default();
                expected.insert(delta.relative_path, state);
            }
            // Both Git and loose workspaces can contain outputs from an earlier exec.
            // The gateway can return regenerated ignored files, so neither Git ignore
            // rules nor the upload extension allowlist define their initial identity.
            // This is only a guard snapshot; the sync upload boundary stays unchanged.
            let base = root.to_path_buf();
            let mut walk = ignore::WalkBuilder::new(root);
            walk.standard_filters(false).filter_entry(move |entry| {
                let rel = entry.path().strip_prefix(&base).unwrap_or(entry.path());
                // Prune the same node caches as gateway seed_cache::walk_files before
                // reading or counting them. Git ignores alone also hide eligible outputs.
                rel.as_os_str().is_empty() || !is_writeback_cache(entry.file_name())
            });
            let mut output_files = 0;
            let mut output_bytes = 0;
            for entry in walk.build() {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => {
                        unscanned_outputs = true;
                        break;
                    }
                };
                // The traversal root has no relative output identity. In particular,
                // a directory symlink root must not mark the empty prefix as unknown.
                if entry.depth() == 0 || entry.file_type().is_some_and(|ty| ty.is_dir()) {
                    continue;
                }
                let rel = entry
                    .path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/");
                if let std::collections::hash_map::Entry::Vacant(slot) = expected.entry(rel.clone())
                {
                    output_files += 1;
                    if output_files > crate::sync::MAX_NON_GIT_WORKSPACE_FILES {
                        // Keep bounded metadata without denying the command. Known source
                        // identities remain usable; unvisited outputs cannot be proved new.
                        unscanned_outputs = true;
                        break;
                    }
                    // A skipped link (including a directory link) is unknown, not absent.
                    // Never follow it outside this bounded traversal or resurrect its paths.
                    if entry.file_type().is_none_or(|ty| !ty.is_file()) {
                        unknown.insert(rel);
                        continue;
                    }
                    // Gateway changed_since limits returned bytes to 5 MiB, but deletions
                    // have no size limit and a large old output can shrink. Snapshot those
                    // identities when possible. An unreadable or over-budget output is
                    // unknown, not absent; it must not prevent unrelated execution/output.
                    match FileState::read_limited(
                        entry.path(),
                        crate::sync::MAX_NON_GIT_WORKSPACE_BYTES - output_bytes,
                    ) {
                        Ok(Some(state)) => {
                            output_bytes += state.content.len();
                            slot.insert(state);
                        }
                        Ok(None) | Err(_) => {
                            unknown.insert(rel);
                        }
                    }
                }
            }
        }
        Ok(Self {
            expected,
            unknown,
            unscanned_outputs,
            started,
            formatter,
            pulled: vec![],
            relaid: vec![],
            kept: vec![],
            platform_files: vec![],
        })
    }

    pub fn apply(&mut self, root: &Path, node: &str, files: Vec<FileDelta>) -> Result<()> {
        let mut written = vec![];
        let mut deletions = HashSet::new();
        for delta in files {
            let rel = Path::new(&delta.relative_path);
            if rel.is_absolute()
                || rel
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                continue;
            }
            if rel.components().any(|c| is_writeback_cache(c.as_os_str()))
                || self.unknown.iter().any(|p| rel.starts_with(p))
            {
                self.kept.push(delta.relative_path);
                continue;
            }
            let Ok(current) = self.read_in_batch(root, rel, &deletions) else {
                self.kept.push(delta.relative_path);
                continue;
            };
            if current.as_ref().map(|s| s.content.as_slice()) == delta.content.as_deref() {
                continue; // Identical bytes are a no-op, including an already absent deletion.
            }
            let expected = self.expected.get(&delta.relative_path);
            if current.as_ref() != expected
                || (!self.pulled.contains(&delta.relative_path)
                    && expected.is_some_and(|s| s.modified > self.started))
            {
                self.kept.push(delta.relative_path);
                continue;
            }
            let layout = self.formatter
                && rel.extension().is_some_and(|e| e == "rs")
                && matches!((&current, &delta.content), (Some(old), Some(new)) if layout_only(&old.content, new));
            let marked = current
                .as_ref()
                .is_some_and(|s| platform_dependent(&s.content))
                || delta.content.as_deref().is_some_and(platform_dependent);
            if !layout {
                self.relaid.retain(|rel| rel != &delta.relative_path);
            } else if !self.pulled.contains(&delta.relative_path) {
                self.relaid.push(delta.relative_path.clone());
            }
            if marked {
                self.platform_files.push(delta.relative_path.clone());
            }
            if delta.content.is_none() {
                deletions.insert(delta.relative_path.clone());
            }
            written.push(delta);
        }
        let applied = apply_pulled_files_for(root, node, &written)?;
        for rel in &applied {
            // Our own write is the expected state for another batch, not a local conflict.
            // A deleted file may now be a directory containing this batch's new children.
            if deletions.contains(rel) {
                self.expected.remove(rel);
                continue;
            }
            match FileState::read(&root.join(rel))? {
                Some(state) => {
                    self.expected.insert(rel.clone(), state);
                }
                None => {
                    self.expected.remove(rel);
                }
            }
        }
        self.pulled.extend(applied);
        Ok(())
    }

    fn read_in_batch(
        &self,
        root: &Path,
        rel: &Path,
        deletions: &HashSet<String>,
    ) -> std::io::Result<Option<FileState>> {
        for ancestor in rel.ancestors().skip(1).filter_map(Path::to_str) {
            if let Some(state) = self.expected.get(ancestor) {
                // The gateway orders a parent deletion before its returned children.
                // Only an unchanged ordinary file accepted above makes those children
                // absent in the pending filesystem. ENOTDIR alone proves nothing.
                if deletions.contains(ancestor) && !state.symlink {
                    return Ok(None);
                }
                return Err(std::io::Error::other(
                    "exec writeback ancestor has a local conflict",
                ));
            }
        }
        if self.unscanned_outputs && !self.expected.contains_key(rel.to_str().unwrap_or_default()) {
            return Err(std::io::Error::other(
                "initial output state was not established",
            ));
        }
        FileState::read(&root.join(rel))
    }
}

/// Keep aligned with gateway seed_cache::is_node_cache. These paths never return as
/// exec deltas; including them in the supplementary guard can exhaust its limits.
fn is_writeback_cache(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            ".git"
                | ".prod"
                | "target"
                | "node_modules"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".pytest_cache"
                | ".mypy_cache"
                | ".ruff_cache"
                | ".tox"
                | ".nox"
                | "build"
                | ".build"
                | "dist"
                | ".cache"
                | ".next"
                | ".turbo"
                | "coverage"
                | "DerivedData"
                | ".swiftpm"
                | ".gradle"
                | ".prod-code-last-used"
                | ".prod-code-stale"
        )
    )
}
