/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::ModulePath;
use crate::fixture::builder::lexer::Source;
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn roots(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut roots = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir.join("tests")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .replace('-', "_");
            if path.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
                roots.push((canonical(&path), name));
            } else if path.join("main.rs").is_file() {
                roots.push((canonical(&path.join("main.rs")), name));
            }
        }
    }
    roots.sort();
    roots
}

/// External, top-level module declarations, including `#[path = "..."]`. Skip item bodies
/// so a local `mod` or a comment cannot be mistaken for a child of the test crate.
fn children(file: &Path, is_root: bool) -> Result<Vec<(String, PathBuf)>> {
    let text = std::fs::read_to_string(file)?;
    let source = Source::new(&text)?;
    let parent = file.parent().context("module has no parent directory")?;
    let module_dir = if is_root || file.file_name().is_some_and(|name| name == "mod.rs") {
        parent.to_path_buf()
    } else {
        file.with_extension("")
    };
    let mut out = Vec::new();
    let mut path = None;
    let mut i = 0;
    while i < source.tokens.len() {
        if source.is(i, "#") && source.is(i + 1, "[") {
            let Some(close) = source.close_of(i + 1) else {
                break;
            };
            if source.is(i + 2, "path") && source.is(i + 3, "=") && i + 4 < close {
                path = serde_json::from_str::<String>(source.t(i + 4)).ok();
            }
            i = close + 1;
        } else if source.is(i, "mod") && source.is(i + 2, ";") {
            let name = source.t(i + 1).to_string();
            let child = match path.take() {
                Some(path) => parent.join(path),
                None => {
                    let ordinary = module_dir.join(format!("{name}.rs"));
                    let directory = module_dir.join(&name).join("mod.rs");
                    if directory.exists() && !ordinary.exists() {
                        directory
                    } else {
                        ordinary
                    }
                }
            };
            out.push((name, canonical(&child)));
            i += 3;
        } else if source.is(i, "{") {
            i = source
                .close_of(i)
                .map_or(source.tokens.len(), |end| end + 1);
            path = None;
        } else {
            if source.is(i, ";") {
                path = None;
            }
            i += 1;
        }
    }
    Ok(out)
}

fn resolve(file: &Path, dir: &Path) -> Result<Option<(PathBuf, ModulePath)>> {
    if !file.starts_with(dir.join("tests")) {
        return Ok(None);
    }
    let roots = roots(dir);
    if let Some((root, name)) = roots.iter().find(|(root, _)| root == file) {
        return Ok(Some((
            root.clone(),
            ModulePath {
                krate: name.clone(),
                segments: Vec::new(),
            },
        )));
    }
    for (root, name) in &roots {
        let mut pending = vec![(root.clone(), Vec::new())];
        let mut seen = BTreeSet::new();
        while let Some((path, segments)) = pending.pop() {
            if path == file {
                return Ok(Some((
                    root.clone(),
                    ModulePath {
                        krate: name.clone(),
                        segments,
                    },
                )));
            }
            if !seen.insert(path.clone()) || !path.is_file() {
                continue;
            }
            for (name, child) in children(&path, path == *root)? {
                let mut segments = segments.clone();
                segments.push(name);
                pending.push((child, segments));
            }
        }
    }
    anyhow::bail!(
        "{} is not a declared module of an integration-test target; add a `mod` declaration \
         (with `#[path = \"...\"]` for a custom file location) in that test crate before moving",
        file.display()
    )
}

pub(super) fn module_of(file: &Path, dir: &Path) -> Result<Option<ModulePath>> {
    Ok(resolve(file, dir)?.map(|(_, module)| module))
}

pub(super) fn parent_module_file(target: &Path) -> Option<PathBuf> {
    let dir = target
        .ancestors()
        .skip(1)
        .find(|dir| dir.join("Cargo.toml").is_file())?;
    if !target.starts_with(dir.join("tests")) {
        return None;
    }
    for (root, _) in roots(dir) {
        let mut pending = vec![root.clone()];
        let mut seen = BTreeSet::new();
        while let Some(file) = pending.pop() {
            if !seen.insert(file.clone()) || !file.is_file() {
                continue;
            }
            for (_, child) in children(&file, file == root).ok()? {
                if child == target {
                    return Some(file);
                }
                pending.push(child);
            }
        }
    }
    None
}

pub(super) fn ensure_same_crate(
    from: &Path,
    to: &Path,
    from_dir: &Path,
    to_dir: &Path,
) -> Result<()> {
    let from = resolve(&canonical(from), from_dir)?.map(|(root, _)| root);
    let to = resolve(&canonical(to), to_dir)?.map(|(root, _)| root);
    anyhow::ensure!(
        from == to,
        "integration-test targets are separate crates; move within the same test crate"
    );
    Ok(())
}
