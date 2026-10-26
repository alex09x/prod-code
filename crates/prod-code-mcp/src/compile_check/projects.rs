/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

mod diagnostics;
mod modules;
mod paths;
mod targets;
mod workspace;

use crate::shadow::HypothesisEdit;
pub(super) use diagnostics::{qualify_short_error, strip_ansi_codes};
use paths::{join_checked, normalize_rel_path, slash_path};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use toml::Value;

struct ManifestOverlay<'a> {
    root: PathBuf,
    proposed: BTreeMap<PathBuf, Option<&'a str>>,
    files: BTreeSet<PathBuf>,
}

impl<'a> ManifestOverlay<'a> {
    fn new(root: &Path, edits: &'a [HypothesisEdit]) -> anyhow::Result<Self> {
        let root = std::fs::canonicalize(root)?;
        let mut files = BTreeSet::new();
        inventory(&root, Path::new(""), &mut files)?;
        let mut proposed = BTreeMap::new();
        for edit in edits {
            let path = normalize_rel_path(Path::new(&edit.relative_path));
            if edit.text.is_some() {
                files.insert(path.clone());
            } else {
                files.remove(&path);
            }
            proposed.insert(path, edit.text.as_deref());
        }
        Ok(Self {
            root,
            proposed,
            files,
        })
    }

    fn read(&self, rel: &Path) -> Option<String> {
        if let Some(entry) = self.proposed.get(rel) {
            return (*entry).map(str::to_string);
        }
        std::fs::read_to_string(self.root.join(rel)).ok()
    }

    fn read_manifest(&self, dir: &Path) -> Option<String> {
        self.read(&normalize_rel_path(&dir.join("Cargo.toml")))
    }

    fn parse_manifest(&self, dir: &Path) -> Option<Value> {
        toml::from_str(&self.read_manifest(dir)?).ok()
    }
}

/// Inspect the checkout, never build products or another checkout through a symlink.
fn inventory(root: &Path, dir: &Path, files: &mut BTreeSet<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(root.join(dir))? {
        let entry = entry?;
        let name = entry.file_name();
        let kind = entry.file_type()?;
        let rel = dir.join(&name);
        if kind.is_dir() {
            if name != ".git" && name != "target" && name != "node_modules" {
                inventory(root, &rel, files)?;
            }
        } else if kind.is_file() {
            files.insert(rel);
        }
    }
    Ok(())
}

pub(super) struct ProjectPlan {
    pub groups: Vec<Option<String>>,
    pub incomplete: Vec<String>,
}

/// Select every group with a supported Cargo target/module edge to an edited source.
/// Directory ancestry alone is not evidence that Cargo will compile a Rust file.
pub(super) fn affected_package_groups(root: &Path, edits: &[HypothesisEdit]) -> ProjectPlan {
    let mut plan = ProjectPlan {
        groups: Vec::new(),
        incomplete: Vec::new(),
    };
    let overlay = match ManifestOverlay::new(root, edits) {
        Ok(overlay) => overlay,
        Err(_) => {
            plan.incomplete
                .push("incomplete compiler verification: cannot inventory checkout".into());
            return plan;
        }
    };
    let manifests: Vec<_> = overlay
        .files
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "Cargo.toml"))
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();
    let mut owners: BTreeMap<PathBuf, BTreeMap<PathBuf, bool>> = BTreeMap::new();
    for dir in &manifests {
        let Some(doc) = overlay.parse_manifest(dir) else {
            continue;
        };
        if doc.get("package").is_none() {
            continue;
        }
        for (target, proven) in targets::roots(&overlay, dir, &doc) {
            for (source, proven) in modules::sources(&overlay, &target, proven) {
                let entry = owners
                    .entry(source)
                    .or_default()
                    .entry(dir.clone())
                    .or_default();
                *entry |= proven;
            }
        }
    }
    for edit in edits {
        let rel = normalize_rel_path(Path::new(&edit.relative_path));
        // An explicit target or #[path] module need not have a `.rs` suffix.
        let packages = if let Some(packages) = owners.get(&rel) {
            packages.clone()
        } else if rel.file_name().is_some_and(|name| name == "Cargo.toml") {
            workspace::nearest_manifest_dir(&overlay, rel.parent().unwrap_or(Path::new("")))
                .into_iter()
                .map(|dir| (dir, true))
                .collect()
        } else if rel.extension().is_none_or(|ext| ext != "rs") {
            // `cargo check` says nothing about other languages; they never made it incomplete.
            continue;
        } else {
            BTreeMap::new()
        };
        if packages.is_empty() {
            plan.incomplete.push(format!("incomplete compiler verification: no supported Cargo target/module ownership for {}", slash_path(&rel)));
        }
        let mut resolved = BTreeMap::new();
        for (package, proven) in packages {
            let dir = workspace::resolve_package_group_dir(&overlay, &package);
            let group = (!dir.as_os_str().is_empty()).then(|| slash_path(&dir));
            *resolved.entry(group).or_insert(false) |= proven;
        }
        for (group, proven) in resolved {
            if !proven {
                plan.incomplete.push(format!("incomplete compiler verification: conditional Cargo target/module ownership for {} in {}", slash_path(&rel), group.as_deref().unwrap_or(".")));
            } else if !plan.groups.contains(&group) {
                plan.groups.push(group);
            }
        }
    }
    // Without any Cargo package there is nothing to scope by: check the root workspace, as
    // before. The same holds when only non-Rust files changed.
    if manifests.is_empty() {
        plan.incomplete.clear();
    }
    if edits.is_empty() || (plan.groups.is_empty() && plan.incomplete.is_empty()) {
        plan.groups.push(None);
    }
    plan
}
