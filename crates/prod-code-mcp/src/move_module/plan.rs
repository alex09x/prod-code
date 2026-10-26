/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::move_item::{
    ModulePath,
    ownership::{Graph, Owner, filesystem},
};
use anyhow::{Context, Result};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

pub(super) struct Transfer {
    pub old: Owner,
    pub parent: Owner,
    pub destination: Owner,
}

pub(super) struct Plan {
    pub graph: Graph,
    pub from: ModulePath,
    pub to: ModulePath,
    pub transfers: Vec<Transfer>,
    pub moves: Vec<(PathBuf, PathBuf)>,
}

impl Plan {
    pub fn new(root: &Path, file: &Path, target: &Path) -> Result<Self> {
        filesystem::no_links(root, file)?;
        filesystem::no_links(root, target)?;
        anyhow::ensure!(
            target.extension().is_some_and(|e| e == "rs"),
            "target is not a Rust module file; nothing was written"
        );
        anyhow::ensure!(file.is_file(), "source is not a regular module file");
        anyhow::ensure!(!target.exists(), "target module file already exists");
        let graph = Graph::load(root, file)?;
        let sources = graph.owners(file, None)?;
        let from = sources[0].module.clone();
        let name = from
            .segments
            .last()
            .context("the crate root is not a module that can move")?;
        let mod_rs = file.file_name().is_some_and(|n| n == "mod.rs");
        let new_file = if mod_rs {
            target.with_extension("").join("mod.rs")
        } else {
            target.to_path_buf()
        };
        filesystem::no_links(root, &new_file)?;
        anyhow::ensure!(!new_file.exists(), "destination module file already exists");
        let dest_target = if graph
            .nodes
            .iter()
            .any(|n| n.file == new_file && n.scope.start == 0)
        {
            new_file.as_path()
        } else {
            target
        };
        let (destinations, parents) = graph.destination(dest_target, &sources)?;
        let to = destinations[0].module.clone();
        anyhow::ensure!(
            to.segments.last() == Some(name),
            "the module keeps its name `{name}` when it moves"
        );
        anyhow::ensure!(
            from.segments != to.segments,
            "the module is already at its destination"
        );
        let moved_roots: BTreeSet<_> = sources.iter().map(|n| n.identity.clone()).collect();
        let mut transfers = Vec::new();
        for old in sources {
            let mut destination = destinations
                .iter()
                .find(|n| n.identity == old.identity)
                .context("destination Cargo ownership is ambiguous")?
                .clone();
            let parent = if let Some(parent) = parents.iter().find(|p| p.identity == old.identity) {
                parent.clone()
            } else {
                graph.parent(&destination)?
            };
            anyhow::ensure!(
                !parent.module.segments.starts_with(&from.segments),
                "a module cannot move into itself"
            );
            destination.file = new_file.clone();
            transfers.push(Transfer {
                old,
                parent,
                destination,
            });
        }
        let old_dir = if mod_rs {
            file.parent().unwrap().to_path_buf()
        } else {
            file.with_extension("")
        };
        let new_dir = if mod_rs {
            new_file.parent().unwrap().to_path_buf()
        } else {
            new_file.with_extension("")
        };
        let mut moves = vec![(file.to_path_buf(), new_file)];
        if old_dir.exists() {
            anyhow::ensure!(
                !new_dir.exists(),
                "destination module directory already exists"
            );
            for child in filesystem::files(root, &old_dir)? {
                if child != file {
                    moves.push((child.clone(), new_dir.join(child.strip_prefix(&old_dir)?)));
                }
            }
        }
        for (old, new) in &moves {
            filesystem::no_links(root, new)?;
            for owner in graph
                .nodes
                .iter()
                .filter(|n| n.file == *old && n.scope.start == 0)
            {
                anyhow::ensure!(
                    !owner.cfg_path
                        && moved_roots.contains(&owner.identity)
                        && owner.module.segments.starts_with(&from.segments),
                    "moved descendant has a shared module owner outside the subtree; nothing was written"
                );
            }
        }
        graph.ensure_unaliased(root, moves.iter().map(|(old, _)| old.as_path()))?;
        super::descendants::ensure_preserved(&graph, &moves, &transfers)?;
        Ok(Self {
            graph,
            from,
            to,
            transfers,
            moves,
        })
    }
}
