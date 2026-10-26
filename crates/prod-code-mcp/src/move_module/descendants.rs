/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Check descendant bindings against the complete filesystem plan before any edits.

use crate::move_item::ownership::{Graph, Owner, normalize};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

fn module_dir(owner: &Owner, new_file: &Path, transfers: &[super::plan::Transfer]) -> PathBuf {
    // Shared destination files receive one declaration. If any owning root needs
    // an attribute, that attribute also determines child lookup for the other roots.
    let merged_attribute = transfers
        .iter()
        .find(|t| t.old.identity == owner.identity && t.old.file == owner.file)
        .is_some_and(|transfer| {
            transfers.iter().any(|t| {
                t.parent.file == transfer.parent.file
                    && (t
                        .old
                        .declaration
                        .as_ref()
                        .is_some_and(|(_, d)| d.path.is_some())
                        || t.destination
                            .declaration
                            .as_ref()
                            .is_some_and(|(_, d)| d.path.is_some())
                        || (t.destination.file
                            != t.parent.module_dir.join(format!(
                                "{}.rs",
                                t.destination.module.segments.last().unwrap()
                            ))
                            && t.destination.file
                                != t.parent
                                    .module_dir
                                    .join(t.destination.module.segments.last().unwrap())
                                    .join("mod.rs")))
            })
        });
    if new_file.file_name().is_some_and(|n| n == "mod.rs")
        || merged_attribute
        || owner
            .declaration
            .as_ref()
            .is_some_and(|(_, d)| d.path.is_some())
    {
        new_file.parent().unwrap().to_path_buf()
    } else {
        new_file.with_extension("")
    }
}

pub(super) fn ensure_preserved(
    graph: &Graph,
    moves: &[(PathBuf, PathBuf)],
    transfers: &[super::plan::Transfer],
) -> Result<()> {
    let destination = |file: &Path| {
        moves
            .iter()
            .find(|(old, _)| old == file)
            .map(|(_, new)| new)
    };
    for child in &graph.nodes {
        let Some((containing_file, decl)) = &child.declaration else {
            continue;
        };
        if decl.inline.is_some() {
            continue;
        }
        let Some(new_file) = destination(containing_file) else {
            continue;
        };
        anyhow::ensure!(
            !child.cfg_path,
            "moved descendant ownership depends on cfg_attr path configuration; nothing was written"
        );
        // An external descendant also keeps lexical paths whose meaning would change
        // when its parent moves. The whole-file transfer cannot safely leave it behind.
        let expected = destination(&child.file).context(
            "descendant module lies outside the moved files; its path and lexical scope cannot be preserved; nothing was written",
        )?;
        let parent = graph.parent(child)?;
        let file_owner = graph
            .nodes
            .iter()
            .find(|n| n.identity == parent.identity && n.file == parent.file && n.scope.start == 0)
            .context("moved file has no lexical root owner")?;
        let new_dir = module_dir(file_owner, new_file, transfers)
            .join(parent.module_dir.strip_prefix(&file_owner.module_dir)?);
        let resolved = if let Some(path) = &decl.path {
            let base = if parent.scope.start == 0 {
                new_file.parent().unwrap()
            } else {
                &new_dir
            };
            normalize(&base.join(path))
        } else if child.file.file_name().is_some_and(|n| n == "mod.rs") {
            new_dir.join(&decl.name).join("mod.rs")
        } else {
            new_dir.join(format!("{}.rs", decl.name))
        };
        anyhow::ensure!(
            resolved == *expected,
            "descendant module path would bind to a different file after moving; nothing was written"
        );
    }
    Ok(())
}
