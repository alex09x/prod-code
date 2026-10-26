/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use ra_ap_syntax::{
    AstNode, Edition, SourceFile, SyntaxNode, ast,
    ast::{HasAttrs, HasName},
};
use std::path::{Path, PathBuf};

/// A source file's lexical root is not necessarily its crate's namespace.
/// Unknown/multiple targets require semantic confirmation before resolving crate::.
pub(super) enum CrateRoot {
    CurrentFile,
    OtherFile {
        root: SyntaxNode,
        parent: Option<SyntaxNode>,
    },
    Unknown,
}

impl CrateRoot {
    pub(super) fn node(&self, context: &SyntaxNode) -> Option<SyntaxNode> {
        match self {
            Self::CurrentFile => context.ancestors().last(),
            Self::OtherFile { root, .. } => Some(root.clone()),
            Self::Unknown => None,
        }
    }

    /// Only a verified module/file link can supply an out-of-line source parent.
    pub(super) fn parent(&self) -> Option<SyntaxNode> {
        match self {
            Self::OtherFile { parent, .. } => parent.clone(),
            _ => None,
        }
    }

    pub(super) fn for_file(path: &Path, edition: Edition) -> anyhow::Result<Self> {
        let Some(directory) = path
            .ancestors()
            .skip(1)
            .find(|p| p.join("Cargo.toml").is_file())
        else {
            return Ok(Self::Unknown);
        };
        let manifest: toml::Value =
            toml::from_str(&std::fs::read_to_string(directory.join("Cargo.toml"))?)?;
        let mut roots = Vec::new();
        for default in ["src/lib.rs", "src/main.rs"] {
            let target = directory.join(default);
            if target.is_file() {
                roots.push(target);
            }
        }
        if let Some(target) = manifest.get("lib") {
            let Some(path) = target.get("path").and_then(toml::Value::as_str) else {
                return Ok(Self::Unknown);
            };
            roots.retain(|p| p != &directory.join("src/lib.rs"));
            roots.push(directory.join(path));
        }
        for kind in ["bin", "test", "example", "bench"] {
            if let Some(targets) = manifest.get(kind).and_then(toml::Value::as_array) {
                for target in targets {
                    let Some(path) = target.get("path").and_then(toml::Value::as_str) else {
                        return Ok(Self::Unknown);
                    };
                    roots.push(directory.join(path));
                }
            }
        }
        // Do not guess target ownership for Cargo's automatic target discovery or
        // build-script crates. The analyzer can still identify their owners exactly.
        if ["src/bin", "tests", "examples", "benches"]
            .iter()
            .any(|p| directory.join(p).is_dir())
            || directory.join("build.rs").is_file()
            || manifest
                .get("package")
                .and_then(|p| p.get("build"))
                .is_some_and(|v| v.as_bool() != Some(false))
        {
            return Ok(Self::Unknown);
        }
        let roots: std::collections::BTreeSet<PathBuf> = roots
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect();
        if roots.len() != 1 {
            return Ok(Self::Unknown);
        }
        let root = roots.first().unwrap();
        if &path.canonicalize()? == root {
            return Ok(Self::CurrentFile);
        }
        let source = std::fs::read_to_string(root)?;
        let parsed = SourceFile::parse(&source, edition);
        if !parsed.errors().is_empty() {
            return Ok(Self::Unknown);
        }
        let root_node = parsed.tree().syntax().clone();
        let parent = module_parent(
            &root_node,
            root.parent().unwrap(),
            &path.canonicalize()?,
            edition,
            0,
        );
        Ok(Self::OtherFile {
            root: root_node,
            parent,
        })
    }
}

/// Follow ordinary module declarations, retaining the actual parent syntax scope.
/// Ambiguous files, path attributes and excessive depth remain semantically unknown.
fn module_parent(
    scope: &SyntaxNode,
    directory: &Path,
    target: &Path,
    edition: Edition,
    depth: usize,
) -> Option<SyntaxNode> {
    if depth > 16 {
        return None;
    }
    let mut matches = Vec::new();
    for module in scope.children().filter_map(ast::Module::cast) {
        if module.attrs().next().is_some() {
            continue;
        }
        let Some(name) = module.name() else {
            continue;
        };
        let name = name.text();
        let child_dir = directory.join(name);
        if let Some(items) = module.item_list() {
            if let Some(parent) =
                module_parent(items.syntax(), &child_dir, target, edition, depth + 1)
            {
                matches.push(parent);
            }
            continue;
        }
        let files = [
            directory.join(format!("{name}.rs")),
            child_dir.join("mod.rs"),
        ];
        let files = files
            .into_iter()
            .filter_map(|p| p.canonicalize().ok())
            .collect::<Vec<_>>();
        if files.len() != 1 {
            continue;
        }
        if files[0] == target {
            matches.push(scope.clone());
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&files[0]) else {
            continue;
        };
        let parsed = SourceFile::parse(&source, edition);
        if !parsed.errors().is_empty() {
            continue;
        }
        if let Some(parent) = module_parent(
            parsed.tree().syntax(),
            &child_dir,
            target,
            edition,
            depth + 1,
        ) {
            matches.push(parent);
        }
    }
    (matches.len() == 1).then(|| matches.remove(0))
}
