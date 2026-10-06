/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Per-session buffer overlay management and session switching.

use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::RustEngine;
use crate::vfs::normalize_vfs_path;

impl RustEngine {
    /// Records `text` (or a deletion) as `session`'s private view of `path`. The database is
    /// updated immediately when nobody else's buffer currently occupies that path.
    pub fn set_session_overlay(
        &mut self,
        session: u64,
        path: &Path,
        text: Option<String>,
    ) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        if !self.overlays.base.contains_key(&norm) {
            let base = self.current_db_text(&norm);
            self.overlays.base.insert(norm.clone(), base);
        }
        self.overlays
            .sessions
            .entry(session)
            .or_default()
            .insert(norm.clone(), text.clone());
        match self.overlays.owner.get(&norm) {
            Some(owner) if *owner != session => Ok(()),
            _ => {
                self.apply_text(&norm, text)?;
                self.overlays.owner.insert(norm, session);
                Ok(())
            }
        }
    }

    /// Drops `session`'s buffer for `path`, restoring the shared base text if that buffer was
    /// the one in the database.
    pub fn clear_session_overlay(&mut self, session: u64, path: &Path) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        let removed = self
            .overlays
            .sessions
            .get_mut(&session)
            .map(|files| files.remove(&norm).is_some())
            .unwrap_or(false);
        if !removed {
            return Ok(());
        }
        if self.overlays.owner.get(&norm) == Some(&session) {
            self.restore_base(&norm)?;
        } else {
            let still_referenced = self
                .overlays
                .sessions
                .values()
                .any(|files| files.contains_key(&norm));
            if !still_referenced && !self.overlays.owner.contains_key(&norm) {
                self.overlays.base.remove(&norm);
            }
        }
        if self
            .overlays
            .sessions
            .get(&session)
            .is_some_and(|files| files.is_empty())
        {
            self.overlays.sessions.remove(&session);
        }
        Ok(())
    }

    /// Drops every buffer of `session` (session teardown).
    pub fn clear_session(&mut self, session: u64) -> Result<()> {
        let paths: Vec<PathBuf> = self
            .overlays
            .sessions
            .get(&session)
            .map(|files| files.keys().cloned().collect())
            .unwrap_or_default();
        for norm in paths {
            self.clear_session_overlay(session, &norm)?;
        }
        Ok(())
    }

    /// Whether any session overlays exist in this engine.
    pub fn has_session_overlays(&self) -> bool {
        !self.overlays.sessions.is_empty() || !self.overlays.owner.is_empty()
    }

    /// Whether a specific session currently has overlays registered.
    pub fn session_has_overlays(&self, session: u64) -> bool {
        self.overlays
            .sessions
            .get(&session)
            .is_some_and(|files| !files.is_empty())
    }

    /// Makes the database reflect `session`'s view: its own buffers are applied and every other
    /// session's buffer on a path this session has not opened is replaced by the base text.
    /// Returns the number of files rewritten. Must run before every query of that session, under
    /// the same lock as the query, so no other session can switch the view in between.
    pub fn activate_session(&mut self, session: u64) -> Result<usize> {
        if self.overlays.sessions.is_empty() && self.overlays.owner.is_empty() {
            return Ok(0);
        }
        if self.overlays.owner.is_empty() && !self.overlays.sessions.contains_key(&session) {
            return Ok(0);
        }
        let mut switched = 0;
        let mine: HashMap<PathBuf, Option<String>> = self
            .overlays
            .sessions
            .get(&session)
            .cloned()
            .unwrap_or_default();

        let foreign: Vec<PathBuf> = self
            .overlays
            .owner
            .iter()
            .filter(|(norm, owner)| **owner != session && !mine.contains_key(*norm))
            .map(|(norm, _)| norm.clone())
            .collect();
        for norm in foreign {
            let base = self.overlays.base.get(&norm).cloned().flatten();
            self.apply_text(&norm, base)?;
            self.overlays.owner.remove(&norm);
            switched += 1;
        }

        for (norm, text) in mine {
            if self.overlays.owner.get(&norm) == Some(&session) {
                continue;
            }
            self.apply_text(&norm, text)?;
            self.overlays.owner.insert(norm, session);
            switched += 1;
        }
        Ok(switched)
    }

    /// Records new shared base text for `path` (a workspace sync landed on disk). It is applied
    /// immediately unless some session's buffer currently occupies the path; that session keeps
    /// its view and the new base becomes visible once its buffer is cleared.
    pub fn update_base(&mut self, path: &Path, text: Option<String>) -> Result<()> {
        let norm = normalize_vfs_path(path, &self.workspace_root);
        if self.overlays.base.contains_key(&norm) {
            self.overlays.base.insert(norm.clone(), text.clone());
        }
        if self.overlays.owner.contains_key(&norm) {
            return Ok(());
        }
        self.apply_text(&norm, text)
    }

    /// Drops every buffer of `session` whose path is not in `keep`: a sync that announces the
    /// session's complete dirty set makes any other overlay of that session stale. Returns the
    /// number of buffers dropped.
    pub fn retain_session_overlays(&mut self, session: u64, keep: &[PathBuf]) -> Result<usize> {
        let keep: HashSet<PathBuf> = keep
            .iter()
            .map(|path| normalize_vfs_path(path, &self.workspace_root))
            .collect();
        let stale: Vec<PathBuf> = self
            .overlays
            .sessions
            .get(&session)
            .map(|files| {
                files
                    .keys()
                    .filter(|norm| !keep.contains(*norm))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for norm in &stale {
            self.clear_session_overlay(session, norm)?;
        }
        Ok(stale.len())
    }

    /// Number of buffers `session` currently overlays.
    pub fn session_overlay_count(&self, session: u64) -> usize {
        self.overlays
            .sessions
            .get(&session)
            .map(|files| files.len())
            .unwrap_or(0)
    }
}
