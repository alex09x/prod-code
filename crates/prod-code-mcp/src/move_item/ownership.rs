/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Occurrence-specific Cargo/module ownership used exclusively by Rust moves.

mod destinations;
mod membership;
pub(crate) use membership::same_roots;
pub(crate) mod filesystem;
mod roots;
mod spelling;

use super::{rust_syntax as syntax, types::ModulePath};
use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    ops::Range,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug)]
pub(crate) struct Owner {
    pub root: PathBuf,
    pub identity: String,
    pub manifest: PathBuf,
    pub kind: &'static str,
    pub file: PathBuf,
    pub module: ModulePath,
    pub scope: Range<usize>,
    pub module_dir: PathBuf,
    pub declaration: Option<(PathBuf, syntax::Declaration)>,
    pub test_crate: bool,
    pub cfg_path: bool,
    pub condition: syntax::Condition,
}

pub(crate) struct Graph {
    pub nodes: Vec<Owner>,
}

impl Owner {
    /// Rust resolves path attributes against the file directory at file scope, and
    /// against the lexical module directory inside an inline module.
    pub fn attribute_base(&self) -> &Path {
        if self.scope.start == 0 {
            self.file.parent().expect("module file has a directory")
        } else {
            &self.module_dir
        }
    }
}

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            _ => out.push(part),
        }
    }
    out
}

impl Graph {
    /// Keep linked declarations as ownership evidence without traversing their targets.
    /// A whole-file edit cannot preserve such an alias when its shared source changes.
    pub fn ensure_unaliased<'a>(
        &self,
        root: &Path,
        files: impl IntoIterator<Item = &'a Path>,
    ) -> Result<()> {
        let physical: BTreeSet<_> = files
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        for owner in &self.nodes {
            if filesystem::linked_coverage(root, &owner.file)?
                .iter()
                .any(|(target, directory)| {
                    physical
                        .iter()
                        .any(|p| p == target || (*directory && p.starts_with(target)))
                })
            {
                anyhow::bail!(
                    "linked module shares a file affected by this move; link-preserving edits are unsupported; nothing was written"
                );
            }
        }
        Ok(())
    }

    pub fn parent(&self, owner: &Owner) -> Result<Owner> {
        let (file, decl) = owner
            .declaration
            .as_ref()
            .context("crate root has no parent module")?;
        let mut segments = owner.module.segments.clone();
        segments.pop();
        self.nodes
            .iter()
            .find(|n| {
                n.identity == owner.identity
                    && n.file == *file
                    && n.module.segments == segments
                    && n.scope.start <= decl.span.start
                    && n.scope.end >= decl.span.end
            })
            .cloned()
            .context("module declaration has no known lexical owner")
    }

    pub fn load(root: &Path, file: &Path) -> Result<Self> {
        let mut nodes = Vec::new();
        for target in roots::discover(root, file)? {
            let (crate_root, name, test_crate, identity) =
                (target.file, target.name, target.test, target.identity);
            filesystem::no_links(root, &crate_root)?;
            let text = syntax::read(&crate_root)?;
            let owner = Owner {
                root: crate_root.clone(),
                identity: identity.clone(),
                manifest: target.manifest.clone(),
                kind: target.kind,
                file: crate_root.clone(),
                module: ModulePath {
                    krate: name,
                    segments: Vec::new(),
                },
                scope: 0..text.len(),
                module_dir: crate_root
                    .parent()
                    .context("crate has no directory")?
                    .to_path_buf(),
                declaration: None,
                test_crate,
                cfg_path: false,
                condition: syntax::Condition::default(),
            };
            let mut pending = vec![(owner, BTreeSet::new())];
            while let Some((mut owner, mut ancestors)) = pending.pop() {
                anyhow::ensure!(
                    nodes.len() < 10000,
                    "module ownership graph exceeds bounded traversal; nothing was written"
                );
                let key = (owner.file.clone(), owner.scope.start);
                anyhow::ensure!(
                    ancestors.insert(key),
                    "cyclic module declarations; nothing was written"
                );
                if owner.file.is_file() && filesystem::no_links(root, &owner.file).is_ok() {
                    let text = syntax::read(&owner.file)?;
                    owner.condition = owner.condition.and(&syntax::scope_condition(
                        &text,
                        owner.scope.clone(),
                        syntax::edition(&owner.root),
                    ));
                    for decl in syntax::declarations(
                        &text,
                        owner.scope.clone(),
                        syntax::edition(&owner.root),
                    ) {
                        let mut variants = vec![(
                            decl.path.clone(),
                            owner.cfg_path || decl.unresolvable_path || !decl.cfg_paths.is_empty(),
                        )];
                        for path in &decl.cfg_paths {
                            let candidate = Some(path.clone());
                            if !variants.iter().any(|(p, _)| p == &candidate) {
                                variants.push((candidate, true));
                            }
                        }
                        for (candidate_path, cfg_path) in variants {
                            let mut decl = decl.clone();
                            decl.path = candidate_path;
                            let mut module = owner.module.clone();
                            module.segments.push(decl.name.clone());
                            let child = if let Some(scope) = &decl.inline {
                                Owner {
                                    root: owner.root.clone(),
                                    identity: identity.clone(),
                                    manifest: owner.manifest.clone(),
                                    kind: owner.kind,
                                    condition: owner.condition.and(&decl.condition),
                                    file: owner.file.clone(),
                                    module,
                                    scope: scope.clone(),
                                    module_dir: filesystem::resolve(
                                        root,
                                        &decl.path.as_ref().map_or_else(
                                            || owner.module_dir.join(&decl.name),
                                            |path| owner.attribute_base().join(path),
                                        ),
                                    ),
                                    declaration: Some((owner.file.clone(), decl)),
                                    test_crate,
                                    cfg_path,
                                }
                            } else {
                                let file = if let Some(path) = &decl.path {
                                    // Out-of-line path attributes are relative to the containing file;
                                    // inside an inline module they are relative to that module directory.
                                    owner.attribute_base().join(path)
                                } else {
                                    let ordinary =
                                        owner.module_dir.join(format!("{}.rs", decl.name));
                                    let directory =
                                        owner.module_dir.join(&decl.name).join("mod.rs");
                                    anyhow::ensure!(
                                        !(ordinary.exists() && directory.exists()),
                                        "ambiguous module files for {}; nothing was written",
                                        decl.name
                                    );
                                    if directory.exists() {
                                        directory
                                    } else {
                                        ordinary
                                    }
                                };
                                let file = filesystem::resolve(root, &file);
                                anyhow::ensure!(
                                    normalize(&file).starts_with(root),
                                    "module path leaves the checkout; nothing was written"
                                );
                                let len = if file.is_file()
                                    && filesystem::no_links(root, &file).is_ok()
                                {
                                    syntax::read(&file)?.len()
                                } else {
                                    0
                                };
                                let module_dir = if decl.path.is_some()
                                    || file.file_name().is_some_and(|n| n == "mod.rs")
                                {
                                    file.parent().unwrap().to_path_buf()
                                } else {
                                    file.with_extension("")
                                };
                                Owner {
                                    root: owner.root.clone(),
                                    identity: identity.clone(),
                                    manifest: owner.manifest.clone(),
                                    kind: owner.kind,
                                    condition: owner.condition.and(&decl.condition),
                                    file,
                                    module,
                                    scope: 0..len,
                                    module_dir,
                                    declaration: Some((owner.file.clone(), decl)),
                                    test_crate,
                                    cfg_path,
                                }
                            };
                            // Do not follow symlinks during graph discovery, including directory cycles.
                            if filesystem::no_links(root, &child.file).is_ok() {
                                pending.push((child, ancestors.clone()));
                            } else {
                                nodes.push(child);
                            }
                        }
                    }
                }
                nodes.push(owner);
            }
        }
        Ok(Self { nodes })
    }

    pub fn owners(&self, file: &Path, at: Option<usize>) -> Result<Vec<Owner>> {
        let file = normalize(file);
        let mut owners: Vec<_> = self
            .nodes
            .iter()
            .filter(|n| {
                n.file == file && at.is_none_or(|at| n.scope.start <= at && at <= n.scope.end)
            })
            .cloned()
            .collect();
        if at.is_some() {
            // The innermost occurrence in each Cargo root owns the selected declaration.
            owners.retain(|n| {
                !self.nodes.iter().any(|m| {
                    m.file == n.file
                        && m.identity == n.identity
                        && m.scope.len() < n.scope.len()
                        && at.is_some_and(|at| m.scope.start <= at && at <= m.scope.end)
                })
            });
        } else {
            owners.retain(|n| n.scope.start == 0);
        }
        anyhow::ensure!(
            !owners.is_empty(),
            "{} has no known Cargo/module owner; nothing was written",
            file.display()
        );
        anyhow::ensure!(
            !owners.iter().any(|n| n.cfg_path),
            "module ownership of {} depends on cfg_attr path configuration; nothing was written",
            file.display()
        );
        let first = &owners[0].module.segments;
        anyhow::ensure!(
            owners.iter().all(|n| &n.module.segments == first),
            "physical file has different module identities; nothing was written"
        );
        let roots: BTreeSet<_> = owners.iter().map(|n| &n.identity).collect();
        anyhow::ensure!(
            roots.len() == owners.len(),
            "physical file has multiple occurrences in one Cargo root; nothing was written"
        );
        anyhow::ensure!(
            owners
                .iter()
                .all(|n| syntax::edition(&n.root) == syntax::edition(&owners[0].root)),
            "shared file has incompatible consumer editions; nothing was written"
        );
        Ok(owners)
    }

    /// An existing declaration is identified by its complete resolved path and lexical owner.
    /// Otherwise a new declaration belongs to the source's Cargo roots, never a preferred lib.
    pub fn destination(&self, target: &Path, from: &[Owner]) -> Result<(Vec<Owner>, Vec<Owner>)> {
        if self
            .nodes
            .iter()
            .any(|n| n.file == target && n.scope.start == 0)
        {
            let owners = self.owners(target, None)?;
            same_roots(from, &owners)?;
            return Ok((owners, Vec::new()));
        }
        anyhow::ensure!(
            !target.exists(),
            "existing target has unknown module ownership; nothing was written"
        );
        let mut parents = Vec::new();
        let mut owners = Vec::new();
        let name = target
            .file_stem()
            .and_then(|s| s.to_str())
            .context("target has no Rust module name")?;
        let declaration = format!("mod {name};");
        let edition = syntax::edition(&from[0].root);
        let parsed = ra_ap_syntax::SourceFile::parse(&declaration, edition);
        anyhow::ensure!(
            parsed.errors().is_empty()
                && syntax::declarations(&declaration, 0..declaration.len(), edition).len() == 1,
            "target filename is not a valid Rust module identifier; nothing was written"
        );
        for parent in destinations::parents(self, target, from)? {
            let mut owner = parent.clone();
            owner.module.segments.push(name.to_string());
            owner.file = target.to_path_buf();
            owner.scope = 0..0;
            owner.module_dir = target.with_extension("");
            owner.declaration = None;
            parents.push(parent);
            owners.push(owner);
        }
        anyhow::ensure!(
            !owners.iter().any(|n| n.cfg_path),
            "destination module ownership depends on cfg_attr path configuration; nothing was written"
        );
        same_roots(from, &owners)?;
        anyhow::ensure!(
            owners
                .iter()
                .all(|n| n.module.segments == owners[0].module.segments),
            "destination has different module identities across Cargo roots; nothing was written"
        );
        Ok((owners, parents))
    }
}
