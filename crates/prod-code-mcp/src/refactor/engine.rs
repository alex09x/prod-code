/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::edits::{apply_text_edits, read_existing, text_for_edit};
use super::journal::{Journal, Undo, copy_path_without_following_links, unique_transfer_path};
use super::ops::MultiOp;
use super::uri::contained;

/// The state of an edit being applied across one or more repository checkouts.
#[derive(Default)]
pub struct MultiRun {
    pub journal: Journal,
    /// Absolute paths written, moved or deleted, in order.
    pub touched: Vec<PathBuf>,
    /// Further (root, rel) paths the sync watermark must drop: the files a directory move carried
    /// along and the files an edit created.
    pub also_forget: Vec<(PathBuf, String)>,
    /// What each file held before the edit first touched it, for [`super::history::remember_applied_multi`].
    pub originals: Vec<(PathBuf, Option<Vec<u8>>)>,
    /// The paths moved away or deleted so far.
    pub vacated: Vec<PathBuf>,
}

impl MultiRun {
    pub fn touch(&mut self, path: PathBuf) {
        if !self.touched.contains(&path) {
            self.touched.push(path);
        }
    }

    pub fn original(&mut self, abs: &Path, bytes: Option<Vec<u8>>) {
        if !self.originals.iter().any(|(p, _)| p == abs) {
            self.originals.push((abs.to_path_buf(), bytes));
        }
    }

    pub fn apply(&mut self, ops: &[MultiOp]) -> Result<()> {
        for op in ops {
            match op {
                MultiOp::Text { root, rel, edits } => self.text(root, rel, edits)?,
                MultiOp::Create {
                    root,
                    rel,
                    overwrite,
                    ignore_if_exists,
                } => {
                    let abs = contained(root, rel)?;
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        if *overwrite {
                            self.original(&abs, read_existing(&abs).ok().flatten());
                            self.journal.set_aside(&abs)?;
                        } else if *ignore_if_exists {
                            continue;
                        } else {
                            bail!(
                                "cannot create {rel}: it already exists, and the edit neither \
                                 overwrites nor ignores it"
                            );
                        }
                    } else {
                        self.original(&abs, None);
                    }
                    self.journal.create_parents(&abs)?;
                    self.journal.undo.push(Undo::RemoveFile(abs.clone()));
                    self.touch(abs.clone());
                    std::fs::write(&abs, b"").with_context(|| format!("create {rel}"))?;
                    self.also_forget.push((root.clone(), rel.clone()));
                }
                MultiOp::Rename {
                    from_root,
                    from_rel,
                    to_root,
                    to_rel,
                    overwrite,
                    ignore_if_exists,
                } => {
                    let from_abs = contained(from_root, from_rel)?;
                    let to_abs = contained(to_root, to_rel)?;
                    if from_abs == to_abs {
                        continue;
                    }
                    anyhow::ensure!(
                        std::fs::symlink_metadata(&from_abs).is_ok(),
                        "rename {from_rel} -> {to_rel}: {from_rel} does not exist"
                    );
                    if std::fs::symlink_metadata(&to_abs).is_ok() {
                        if *overwrite {
                            self.original(&to_abs, read_existing(&to_abs).ok().flatten());
                            self.journal.set_aside(&to_abs)?;
                        } else if *ignore_if_exists {
                            continue;
                        } else {
                            bail!(
                                "rename {from_rel} -> {to_rel}: {to_rel} already exists, and the edit does \
                                 not overwrite it"
                            );
                        }
                    }
                    self.touch(from_abs.clone());
                    self.touch(to_abs.clone());
                    self.original(&from_abs, read_existing(&from_abs).ok().flatten());
                    self.original(&to_abs, None);
                    self.journal.create_parents(&to_abs)?;
                    let mut copied_across_volumes = false;
                    match std::fs::rename(&from_abs, &to_abs) {
                        Ok(()) => {
                            self.journal
                                .undo
                                .push(Undo::Move(to_abs.clone(), from_abs.clone()));
                            self.journal.moved(&from_abs, &to_abs);
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
                            let staging = unique_transfer_path(&to_abs);
                            self.journal.undo.push(Undo::RemovePath(staging.clone()));
                            copy_path_without_following_links(&from_abs, &staging).with_context(
                                || format!("copy {from_rel} for cross-volume rename"),
                            )?;
                            std::fs::rename(&staging, &to_abs).with_context(|| {
                                format!("publish {to_rel} for cross-volume rename")
                            })?;
                            self.journal.undo.push(Undo::RemovePath(to_abs.clone()));
                            // Carry aside paths from the source tree to the published copy before
                            // moving the original tree to its same-volume undo location.
                            self.journal.moved(&from_abs, &to_abs);
                            if to_abs.is_dir() {
                                for inner in files_under(&to_abs) {
                                    self.also_forget
                                        .push((from_root.clone(), format!("{from_rel}/{inner}")));
                                    self.also_forget
                                        .push((to_root.clone(), format!("{to_rel}/{inner}")));
                                }
                                for (path, _) in &mut self.originals {
                                    if let Ok(rest) = path.strip_prefix(&from_abs) {
                                        *path = to_abs.join(rest);
                                    }
                                }
                            }
                            self.journal.set_aside(&from_abs).with_context(|| {
                                format!("retire {from_rel} after cross-volume copy")
                            })?;
                            copied_across_volumes = true;
                        }
                        Err(error) => {
                            return Err(error)
                                .with_context(|| format!("rename {from_rel} -> {to_rel}"));
                        }
                    }
                    if !copied_across_volumes && to_abs.is_dir() {
                        for inner in files_under(&to_abs) {
                            self.also_forget
                                .push((from_root.clone(), format!("{from_rel}/{inner}")));
                            self.also_forget
                                .push((to_root.clone(), format!("{to_rel}/{inner}")));
                        }
                        // A file an earlier step rewrote travels with its directory, and so
                        // does what it held before, for the report.
                        for (path, _) in &mut self.originals {
                            if let Ok(rest) = path.strip_prefix(&from_abs) {
                                *path = to_abs.join(rest);
                            }
                        }
                    }
                    self.vacated.push(from_abs);
                }
                MultiOp::Delete {
                    root,
                    rel,
                    recursive,
                } => {
                    let abs = contained(root, rel)?;
                    if std::fs::symlink_metadata(&abs).is_ok() {
                        if abs.is_dir() && !abs.is_symlink() {
                            anyhow::ensure!(
                                *recursive || std::fs::read_dir(&abs)?.next().is_none(),
                                "deleting the directory {rel} needs `recursive: true`, it is \
                                 not empty"
                            );
                            for inner in files_under(&abs) {
                                self.also_forget
                                    .push((root.clone(), format!("{rel}/{inner}")));
                            }
                        } else {
                            self.original(&abs, read_existing(&abs)?);
                        }
                        self.journal.set_aside(&abs)?;
                    }
                    self.vacated.push(abs.clone());
                    self.touch(abs);
                }
            }
        }
        Ok(())
    }

    /// A text edit of `rel` as it is now, after the steps before it.
    pub fn text(&mut self, root: &Path, rel: &str, edits: &[serde_json::Value]) -> Result<()> {
        let abs = contained(root, rel)?;
        let (bytes, current) = text_for_edit(&abs)?;
        if bytes.is_none()
            && let Some(gone) = self.vacated.iter().find(|v| is_abs_at_or_under(&abs, v))
        {
            let gone_display = gone
                .strip_prefix(root)
                .map(|r| r.to_string_lossy().into_owned())
                .unwrap_or_else(|_| gone.display().to_string());
            bail!(
                "edit {rel}: an earlier step of the edit moved or deleted {gone_display}; the changes of a \
                 workspace edit apply in order, so an edit after a move names the new path"
            );
        }
        let new_text = apply_text_edits(&current, edits).with_context(|| format!("edit {rel}"))?;
        self.touch(abs.clone());
        match bytes {
            Some(bytes) => {
                self.journal
                    .undo
                    .push(Undo::Write(abs.clone(), bytes.clone()));
                std::fs::write(&abs, new_text).with_context(|| format!("write {rel}"))?;
                self.original(&abs, Some(bytes));
            }
            None => {
                self.journal.create_parents(&abs)?;
                self.journal.undo.push(Undo::RemoveFile(abs.clone()));
                std::fs::write(&abs, new_text).with_context(|| format!("write {rel}"))?;
                self.original(&abs, None);
            }
        }
        Ok(())
    }
}

pub fn is_abs_at_or_under(path: &Path, under: &Path) -> bool {
    path == under || path.starts_with(under)
}

pub fn forget_synced_across_roots(
    roots: &[PathBuf],
    touched: &[PathBuf],
    also_forget: &[(PathBuf, String)],
) {
    let mut forget_by_root: std::collections::HashMap<PathBuf, Vec<String>> =
        std::collections::HashMap::new();
    for p in touched {
        let mut best: Option<(&PathBuf, &Path)> = None;
        for r in roots {
            if let Ok(rel) = p.strip_prefix(r) {
                match &best {
                    Some((best_r, _)) if r.as_os_str().len() <= best_r.as_os_str().len() => {}
                    _ => best = Some((r, rel)),
                }
            }
        }
        if let Some((r, rel)) = best {
            forget_by_root
                .entry(r.clone())
                .or_default()
                .push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    for (r, rel) in also_forget {
        forget_by_root
            .entry(r.clone())
            .or_default()
            .push(rel.clone());
    }
    for (r, paths) in &forget_by_root {
        crate::sync::forget_synced_files(r, paths);
    }
}

/// The files under the directory `dir`, relative to it, symlinks not followed.
pub fn files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(at) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&at) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => stack.push(path),
                Ok(_) => {
                    if let Ok(rel) = path.strip_prefix(dir) {
                        out.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
                Err(_) => {}
            }
        }
    }
    out
}
