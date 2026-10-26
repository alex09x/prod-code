/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{Graph, Owner};
use anyhow::Result;
use std::{collections::BTreeSet, path::Path};

/// A shared physical parent can have an attributed occurrence in another Cargo
/// root. Reuse that exact occurrence, never a file with a matching basename.
pub(super) fn parents(graph: &Graph, target: &Path, sources: &[Owner]) -> Result<Vec<Owner>> {
    let direct: Vec<Vec<_>> = sources
        .iter()
        .map(|source| {
            let mut candidates: Vec<_> = graph
                .nodes
                .iter()
                .filter(|n| {
                    n.identity == source.identity
                        && target.parent() == Some(n.module_dir.as_path())
                        && n.file.is_file()
                })
                .cloned()
                .collect();
            if candidates.len() > 1 {
                // Attributed siblings can share the root's directory, but do not
                // take ownership of new conventional sibling modules there.
                let conventional: Vec<_> = candidates
                    .iter()
                    .filter(|n| {
                        !n.cfg_path
                            && n.declaration
                                .as_ref()
                                .is_none_or(|(_, d)| d.path.is_none() && d.cfg_paths.is_empty())
                    })
                    .cloned()
                    .collect();
                if !conventional.is_empty() {
                    candidates = conventional;
                } else {
                    // When all occurrences are attributed, only lexical ancestry
                    // can disambiguate them; a sibling's basename is not evidence.
                    let nearest = candidates
                        .iter()
                        .filter(|n| source.module.segments.starts_with(&n.module.segments))
                        .map(|n| n.module.segments.len())
                        .max();
                    if let Some(depth) = nearest {
                        candidates.retain(|n| {
                            source.module.segments.starts_with(&n.module.segments)
                                && n.module.segments.len() == depth
                        });
                    }
                }
            }
            candidates
        })
        .collect();
    let confirmed: BTreeSet<_> = direct
        .iter()
        .filter(|nodes| nodes.len() == 1)
        .map(|nodes| (nodes[0].file.clone(), nodes[0].module.segments.clone()))
        .collect();
    let mut parents = Vec::new();
    for (source, mut candidates) in sources.iter().zip(direct) {
        if candidates.is_empty() {
            candidates = graph
                .nodes
                .iter()
                .filter(|n| {
                    n.identity == source.identity
                        && confirmed.contains(&(n.file.clone(), n.module.segments.clone()))
                })
                .cloned()
                .collect();
        }
        anyhow::ensure!(
            !candidates.is_empty(),
            "target has no unique parent in the source Cargo root: no module declares {}; \
             create the parent module first; nothing was written",
            target.parent().unwrap_or(target).display()
        );
        anyhow::ensure!(
            candidates.len() == 1,
            "target has no unique parent in the source Cargo root; nothing was written"
        );
        parents.push(candidates.remove(0));
    }
    Ok(parents)
}
