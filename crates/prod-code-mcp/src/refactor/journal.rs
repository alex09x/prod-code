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

use anyhow::{Context, Result};

/// One step of an edit taken back.
pub enum Undo {
    /// The file had these bytes.
    Write(PathBuf, Vec<u8>),
    /// The edit created this file.
    RemoveFile(PathBuf),
    /// The edit created a file, directory or symlink tree.
    RemovePath(PathBuf),
    /// The edit created this directory.
    RemoveDir(PathBuf),
    /// The edit moved the first path from the second.
    Move(PathBuf, PathBuf),
}

pub fn remove_path(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

pub fn copy_path_without_following_links(from: &Path, to: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(from)
        .with_context(|| format!("cannot inspect {} for cross-volume rename", from.display()))?;
    if metadata.file_type().is_symlink() {
        let target = std::fs::read_link(from)
            .with_context(|| format!("cannot read symlink {}", from.display()))?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(target, to)?;
        #[cfg(windows)]
        {
            if std::fs::metadata(from).is_ok_and(|target_metadata| target_metadata.is_dir()) {
                std::os::windows::fs::symlink_dir(target, to)?;
            } else {
                std::os::windows::fs::symlink_file(target, to)?;
            }
        }
        #[cfg(not(any(unix, windows)))]
        anyhow::bail!("cross-volume rename of symlinks is unsupported on this platform");
    } else if metadata.is_dir() {
        std::fs::create_dir(to)
            .with_context(|| format!("cannot create directory {}", to.display()))?;
        for entry in std::fs::read_dir(from)
            .with_context(|| format!("cannot read directory {}", from.display()))?
        {
            let entry = entry?;
            copy_path_without_following_links(&entry.path(), &to.join(entry.file_name()))?;
        }
        std::fs::set_permissions(to, metadata.permissions())?;
    } else if metadata.is_file() {
        std::fs::copy(from, to)
            .with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
    } else {
        anyhow::bail!(
            "cross-volume rename cannot copy special file {}",
            from.display()
        );
    }
    Ok(())
}

pub fn unique_transfer_path(destination: &Path) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = destination
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    loop {
        let sequence = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = destination.with_file_name(format!(
            ".{name}.prod-code-transfer-{}-{sequence}",
            std::process::id()
        ));
        if std::fs::symlink_metadata(&candidate).is_err() {
            return candidate;
        }
    }
}

/// How to take back what an edit did so far, and what it set aside to remove once it lands.
#[derive(Default)]
pub struct Journal {
    pub undo: Vec<Undo>,
    pub set_aside: Vec<PathBuf>,
}

impl Journal {
    /// Creates the missing directories above `abs`, remembering each to remove again.
    pub fn create_parents(&mut self, abs: &Path) -> Result<()> {
        let Some(parent) = abs.parent() else {
            return Ok(());
        };
        let mut missing = Vec::new();
        let mut at = parent;
        while std::fs::symlink_metadata(at).is_err() {
            missing.push(at.to_path_buf());
            match at.parent() {
                Some(up) => at = up,
                None => break,
            }
        }
        for dir in missing.into_iter().rev() {
            std::fs::create_dir(&dir)
                .with_context(|| format!("cannot create the directory {}", dir.display()))?;
            self.undo.push(Undo::RemoveDir(dir));
        }
        Ok(())
    }

    /// Moves what is at `abs` out of the way, next to it, until the edit lands or is undone.
    pub fn set_aside(&mut self, abs: &Path) -> Result<()> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let name = abs.file_name().unwrap_or_default().to_string_lossy();
        let aside = loop {
            let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let candidate = abs.with_file_name(format!(
                ".{name}.prod-code-undo-{}-{seq}",
                std::process::id()
            ));
            if std::fs::symlink_metadata(&candidate).is_err() {
                break candidate;
            }
        };
        std::fs::rename(abs, &aside)
            .with_context(|| format!("cannot move {} out of the way", abs.display()))?;
        self.undo.push(Undo::Move(aside.clone(), abs.to_path_buf()));
        self.moved(abs, &aside);
        self.set_aside.push(aside);
        Ok(())
    }

    /// `from` was moved to `to`: what was set aside inside it is inside `to` now, and that is
    /// where it is removed from once the edit lands. The undo steps keep the old paths, which
    /// are right again by the time they run, the move having been taken back first.
    pub fn moved(&mut self, from: &Path, to: &Path) {
        for aside in &mut self.set_aside {
            if let Ok(rest) = aside.strip_prefix(from) {
                *aside = to.join(rest);
            }
        }
    }

    /// The edit landed: what it set aside goes.
    pub fn commit(&mut self) {
        for aside in self.set_aside.drain(..) {
            let removed = if aside.is_dir() && !aside.is_symlink() {
                std::fs::remove_dir_all(&aside)
            } else {
                std::fs::remove_file(&aside)
            };
            if let Err(error) = removed {
                tracing::warn!(path = %aside.display(), %error, "could not remove what an edit replaced");
            }
        }
        self.undo.clear();
    }

    /// Takes every step back, last first. Returns how many were taken back and the ones that
    /// could not be.
    pub fn roll_back(&mut self) -> (usize, Vec<String>) {
        let mut restored = 0;
        let mut failed = Vec::new();
        for step in self.undo.drain(..).rev() {
            let (what, result) = match &step {
                Undo::Write(abs, bytes) => {
                    if std::fs::read(abs).ok().as_deref() == Some(bytes.as_slice()) {
                        continue;
                    }
                    (abs.clone(), std::fs::write(abs, bytes))
                }
                Undo::RemoveFile(abs) => {
                    if !abs.exists() && std::fs::symlink_metadata(abs).is_err() {
                        continue;
                    }
                    (abs.clone(), std::fs::remove_file(abs))
                }
                Undo::RemovePath(abs) => (abs.clone(), remove_path(abs)),
                Undo::RemoveDir(abs) => (abs.clone(), std::fs::remove_dir(abs)),
                Undo::Move(from, to) => (to.clone(), std::fs::rename(from, to)),
            };
            match result {
                Ok(()) => restored += 1,
                Err(err) => failed.push(format!("{}: {err}", what.display())),
            }
        }
        self.set_aside.clear();
        (restored, failed)
    }
}
