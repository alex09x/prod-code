/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::fixture::builder::lexer::{Source, bare};
use anyhow::{Context, Result};

pub(super) use super::module_attributes::constant_cfg;
use super::module_attributes::{self, Attributes};

pub(super) struct Module {
    pub target: PathBuf,
    pub name: String,
    pub segments: Vec<String>,
    pub definite: bool,
    pub library: bool,
}

/// Qualify at the actual occurrence, not once per file: inline modules may import
/// only the owner, and a package's library and binary are separate crates.
#[allow(clippy::too_many_arguments)]
pub(super) async fn spelling(
    remote: std::net::SocketAddr,
    root: &Path,
    home: &Path,
    home_text: &str,
    declaration: &super::types::StructDecl,
    using: &Path,
    using_text: &str,
    occurrence: usize,
    helper: &str,
) -> Result<String> {
    let home_scope = crate::parameter_object::rust_modules(home_text, declaration.start);
    let using_scope = crate::parameter_object::rust_modules(using_text, occurrence);
    if home == using
        && home_scope == using_scope
        && super::binding_path::prefixes(using_text, occurrence, &declaration.name)?.is_empty()
    {
        return Ok(helper.to_owned());
    }
    let homes = modules(home)?;
    let uses = modules(using)?;
    super::occurrence_path::spelling(
        &homes,
        &uses,
        &home_scope,
        &using_scope,
        using_text,
        occurrence,
        &declaration.name,
        helper,
        || async {
            crate::signature::hover_markdown(
                remote,
                root,
                using,
                using_text,
                occurrence,
                &mut std::collections::BTreeMap::new(),
            )
            .await
        },
    )
    .await
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn modules(file: &Path) -> Result<Vec<Module>> {
    // Keep the existing package/layout validation and crate-name spelling.
    let (dir, fallback) = crate::move_item::module_of(file)?;
    let manifest: toml::Value = toml::from_str(&std::fs::read_to_string(dir.join("Cargo.toml"))?)?;
    let package_name = manifest
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(toml::Value::as_str)
        .context("Cargo package has no name")?;
    let mut targets = Vec::new();
    let lib = manifest.get("lib");
    let lib_path = lib
        .and_then(|l| l.get("path"))
        .and_then(toml::Value::as_str)
        .unwrap_or("src/lib.rs");
    if dir.join(lib_path).is_file() {
        let name = lib
            .and_then(|l| l.get("name"))
            .and_then(toml::Value::as_str)
            .unwrap_or(package_name);
        targets.push((dir.join(lib_path), name.replace('-', "_"), true));
    }
    let bins = manifest.get("bin").and_then(toml::Value::as_array);
    let explicit_main = bins.is_some_and(|bins| {
        bins.iter().any(|bin| {
            bin.get("path")
                .and_then(toml::Value::as_str)
                .is_some_and(|p| canonical(&dir.join(p)) == canonical(&dir.join("src/main.rs")))
        })
    });
    if dir.join("src/main.rs").is_file() && !explicit_main {
        targets.push((
            dir.join("src/main.rs"),
            package_name.replace('-', "_"),
            false,
        ));
    }
    if let Some(bins) = bins {
        for bin in bins {
            if let Some(path) = bin.get("path").and_then(toml::Value::as_str) {
                let name = bin
                    .get("name")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(package_name);
                targets.push((dir.join(path), name.replace('-', "_"), false));
            }
        }
    }
    for folder in ["src/bin", "tests", "examples", "benches"] {
        if let Ok(entries) = std::fs::read_dir(dir.join(folder)) {
            for entry in entries.flatten() {
                let path = entry.path();
                let root = if path.is_dir() {
                    path.join("main.rs")
                } else {
                    path.clone()
                };
                if root.is_file() && root.extension().is_some_and(|e| e == "rs") {
                    let name = path
                        .file_stem()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .replace('-', "_");
                    targets.push((root, name, false));
                }
            }
        }
    }
    // Source-only fragments have no target identity to disambiguate. Preserve
    // their existing module proposal and let analyzer diagnostics gate writes.
    // A declared target never takes this fallback.
    if targets.is_empty() {
        return Ok(vec![Module {
            target: canonical(&dir.join("Cargo.toml")),
            name: fallback.krate,
            segments: fallback.segments,
            definite: true,
            library: false,
        }]);
    }
    let wanted = canonical(file);
    let mut result = Vec::new();
    for (target, name, library) in targets {
        let target = canonical(&target);
        let mut visited = BTreeSet::new();
        // Retain every possible module identity. An unresolved cfg alias is not
        // evidence that its path is active; the occurrence must disambiguate it.
        let definite = locate(&target, &wanted, true, &[], &mut visited, false)?;
        visited.clear();
        let possible = locate(&target, &wanted, true, &[], &mut visited, true)?;
        for segments in possible {
            result.push(Module {
                target: target.clone(),
                name: name.clone(),
                definite: definite.contains(&segments),
                library,
                segments,
            });
        }
    }
    anyhow::ensure!(
        !result.is_empty(),
        "source is not a declared module of a Cargo target; nothing was planned"
    );
    Ok(result)
}

fn locate(
    file: &Path,
    wanted: &Path,
    root: bool,
    segments: &[String],
    visited: &mut BTreeSet<PathBuf>,
    conditional: bool,
) -> Result<Vec<Vec<String>>> {
    if file == wanted {
        return Ok(vec![segments.to_vec()]);
    }
    if !visited.insert(file.to_path_buf()) || !file.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(file)?;
    let source = Source::new(&text)?;
    let parent = file.parent().context("module has no parent")?;
    let dir = if root || file.file_name().is_some_and(|n| n == "mod.rs") {
        parent.to_path_buf()
    } else {
        file.with_extension("")
    };
    let found = children(
        &source,
        0,
        source.tokens.len(),
        parent,
        &dir,
        wanted,
        segments,
        visited,
        conditional,
    );
    visited.remove(file);
    found
}

// Follow only module declarations in this package, reusing the structural lexer.
// This is target membership, not dependency resolution or a general Rust parser.
#[allow(clippy::too_many_arguments)]
fn children(
    source: &Source<'_>,
    mut at: usize,
    end: usize,
    parent: &Path,
    dir: &Path,
    wanted: &Path,
    segments: &[String],
    visited: &mut BTreeSet<PathBuf>,
    conditional: bool,
) -> Result<Vec<Vec<String>>> {
    let mut result = Vec::new();
    let mut attributes = vec![Attributes::default()];
    while at < end {
        if source.is(at, "#") && source.is(at + 1, "[") {
            let close = source
                .close_of(at + 1)
                .context("unclosed module attribute")?;
            attributes = module_attributes::apply(source, at + 2, close, attributes)?;
            at = close + 1;
        } else if source.is(at, "mod") && (source.is(at + 2, ";") || source.is(at + 2, "{")) {
            let name = source.t(at + 1).to_owned();
            // Raw spelling belongs in Rust paths, never filesystem components.
            let filename = bare(&name);
            let mut nested = segments.to_vec();
            nested.push(name.clone());
            let next = if source.is(at + 2, "{") {
                source.close_of(at + 2).context("unclosed inline module")? + 1
            } else {
                at + 3
            };
            for attribute in std::mem::replace(&mut attributes, vec![Attributes::default()]) {
                if attribute.disabled || (attribute.uncertain && !conditional) {
                    continue;
                }
                let found = if source.is(at + 2, "{") {
                    let close = source.close_of(at + 2).context("unclosed inline module")?;
                    children(
                        source,
                        at + 3,
                        close,
                        &dir.join(filename),
                        &dir.join(filename),
                        wanted,
                        &nested,
                        visited,
                        conditional,
                    )?
                } else {
                    let child = if let Some(path) = attribute.path {
                        parent.join(path)
                    } else {
                        let ordinary = dir.join(format!("{filename}.rs"));
                        if ordinary.is_file() {
                            ordinary
                        } else {
                            dir.join(filename).join("mod.rs")
                        }
                    };
                    locate(
                        &canonical(&child),
                        wanted,
                        false,
                        &nested,
                        visited,
                        conditional,
                    )?
                };
                result.extend(found);
            }
            at = next;
        } else if source.is(at, "{") {
            at = source.close_of(at).map_or(end, |close| close + 1);
            attributes = vec![Attributes::default()];
        } else {
            if source.is(at, ";") {
                attributes = vec![Attributes::default()];
            }
            at += 1;
        }
    }
    Ok(result)
}
