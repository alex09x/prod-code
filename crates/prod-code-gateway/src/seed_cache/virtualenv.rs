/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Whether `dir` is a Python virtual environment: it holds `pyvenv.cfg`.
pub fn is_virtualenv(dir: &std::path::Path) -> bool {
    dir.join("pyvenv.cfg").is_file()
}

/// The `node_modules` trees and Python virtual environments of the copy at `root`, relative to
/// it: at the root and in the packages below it, never one inside another (that is part of its
/// parent), nor any under `.git` or `target`.
pub(crate) fn dependency_trees(root: &std::path::Path) -> Vec<PathBuf> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            match entry.file_name().to_str() {
                Some(".git" | "target") => {}
                Some("node_modules") => found.extend(path.strip_prefix(root).ok().map(Into::into)),
                _ if is_virtualenv(&path) => {
                    found.extend(path.strip_prefix(root).ok().map(Into::into))
                }
                _ => walk(root, &path, found),
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found);
    found.sort();
    found
}

/// Copies the seed copy's `node_modules` trees and virtual environments into the new copy at
/// `to`, when there are any and they fit (`seed_fits`, #412, #414, #419). Without them every
/// import from a dependency resolves to nothing in the new
/// worktree until something installs the packages again. `cp -a` keeps the symlinks of `.bin`,
/// of pnpm's layout and of a venv (`lib64 -> lib`, `bin/python`); a venv's scripts are then
/// rewritten to name the copy. The trees are the worktree's own afterwards, so an install in one
/// worktree never changes another's.
pub fn seed_dependency_trees(
    from: &std::path::Path,
    to: &std::path::Path,
) -> std::io::Result<Option<u64>> {
    seed_dependency_trees_within(from, to, disk_space(to))
}

pub(crate) fn seed_dependency_trees_within(
    from: &std::path::Path,
    to: &std::path::Path,
    space: Option<DiskSpace>,
) -> std::io::Result<Option<u64>> {
    let trees = dependency_trees(from);
    if trees.is_empty() {
        return Ok(None);
    }
    let size: u64 = trees.iter().map(|rel| tree_size(&from.join(rel))).sum();
    if !seed_fits("node_modules and virtual environments", size, space) {
        return Ok(None);
    }
    for rel in trees {
        let dest = to.join(&rel);
        let Some(parent) = dest.parent() else {
            continue;
        };
        std::fs::create_dir_all(parent)?;
        let status = std::process::Command::new("cp")
            .arg("-a")
            .arg(from.join(&rel))
            .arg(parent)
            .status()?;
        if !status.success() {
            return Err(std::io::Error::other(format!(
                "copying {} failed: {status}",
                from.join(&rel).display()
            )));
        }
        if is_virtualenv(&dest) {
            relocate_virtualenv(&from.join(&rel), &dest)?;
            let _ = prewarm_virtualenv_pycache(&dest);
        }
    }
    Ok(Some(size))
}

/// Rewrites the scripts in the `bin` of a virtual environment copied from `old` to `new` that
/// name `old` (console-script shebangs, `activate`) so that they name `new`: otherwise running
/// the copy's `pytest` would start the other worktree's interpreter (#414). Binaries and files
/// over 1 MiB are left alone. Returns how many scripts were rewritten.
pub fn relocate_virtualenv(old: &std::path::Path, new: &std::path::Path) -> std::io::Result<usize> {
    let (Some(old_text), Some(new_text)) = (old.to_str(), new.to_str()) else {
        return Ok(0);
    };
    let Ok(entries) = std::fs::read_dir(new.join("bin")) else {
        return Ok(0);
    };
    let mut rewritten = 0;
    for entry in entries.flatten() {
        let small_file = entry.file_type().is_ok_and(|kind| kind.is_file())
            && entry.metadata().is_ok_and(|m| m.len() <= 1 << 20);
        if !small_file {
            continue;
        }
        let path = entry.path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.contains(old_text) {
            // Writing an existing file keeps its mode, so a script stays executable.
            std::fs::write(&path, text.replace(old_text, new_text))?;
            rewritten += 1;
        }
    }
    Ok(rewritten)
}

/// Finds a trusted host Python interpreter on the system (Roadmap 6.2).
pub(crate) fn find_trusted_host_python() -> Option<PathBuf> {
    const CANDIDATES: &[&str] = &[
        "/usr/bin/python3",
        "/usr/local/bin/python3",
        "/opt/homebrew/bin/python3",
        "/usr/bin/python",
    ];
    for &cand in CANDIDATES {
        let p = Path::new(cand);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
    }
    std::env::var_os("PATH").and_then(|path| {
        std::env::split_paths(&path)
            .flat_map(|dir| [dir.join("python3"), dir.join("python")])
            .find(|p| p.is_file())
    })
}

/// Pre-warms Python bytecode (`.pyc` pycache) inside a virtual environment (Roadmap 6.2).
///
/// Uses the host's trusted Python interpreter in isolated mode with site initialization disabled
/// (`-I -S`) to compile all `.py` files in `lib` into bytecode. This eliminates cold import and parse
/// latency without executing untrusted workspace-provided binaries or running untrusted `sitecustomize.py` hooks.
pub fn prewarm_virtualenv_pycache(venv: &std::path::Path) -> std::io::Result<usize> {
    if !is_virtualenv(venv) {
        return Ok(0);
    }
    let lib_dir = venv.join("lib");
    if !lib_dir.is_dir() {
        return Ok(0);
    }
    let host_python = match find_trusted_host_python() {
        Some(p) => p,
        None => return Ok(0),
    };

    match std::process::Command::new(&host_python)
        .args(["-I", "-S", "-m", "compileall", "-q", "-f"])
        .arg(&lib_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            tracing::info!(venv = %venv.display(), "🐍 [PYCACHE] pre-warmed virtual environment bytecode");
            Ok(1)
        }
        Ok(status) => {
            tracing::debug!(venv = %venv.display(), ?status, "compileall completed with non-zero status");
            Ok(0)
        }
        Err(err) => {
            tracing::debug!(venv = %venv.display(), %err, "could not execute host python for compileall; skipping pre-warming");
            Ok(0)
        }
    }
}

