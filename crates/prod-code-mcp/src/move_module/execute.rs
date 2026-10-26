/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::move_item::{add_import, drop_import, rust_syntax};

use super::syntax::*;
use super::types::*;

/// Moves the module whose file is `file` so that its file becomes `target`, with every path to
/// it. Nothing is written; see [`ModuleMove::write`].
pub async fn move_module(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    target: &Path,
) -> Result<ModuleMove> {
    let plan = super::plan::Plan::new(root, file, target)?;
    let (from, to) = (&plan.from, &plan.to);
    let name = from
        .segments
        .last()
        .cloned()
        .context("crate root cannot move")?;
    let old_parents: Vec<_> = plan
        .transfers
        .iter()
        .map(|t| t.old.declaration.as_ref().unwrap().0.clone())
        .collect();
    let new_parents: Vec<_> = plan
        .transfers
        .iter()
        .map(|t| t.parent.file.clone())
        .collect();
    let mut refs = Vec::new();
    for transfer in &plan.transfers {
        let (parent, decl) = transfer
            .old
            .declaration
            .as_ref()
            .context("module has no declaration")?;
        let text = rust_syntax::read(parent)?;
        let (line, col) = crate::signature::position_at(&text, decl.name_at)?;
        refs.extend(
            crate::signature::references(remote, root, parent, line, col)
                .await
                .with_context(|| {
                    format!("cannot find the paths to `{name}`; nothing was planned")
                })?,
        );
    }
    refs.sort();
    refs.dedup();
    let moves = &plan.moves;
    let moved_to = |path: &Path| {
        moves
            .iter()
            .find(|(old, _)| old == path)
            .map(|(_, new)| new.clone())
    };

    // Each file's text as it is now, then rewritten at the positions the analyzer gave.
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut notes = Vec::new();
    let mut unmatched = Vec::new();
    let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32)>> = BTreeMap::new();
    for (path, l, c) in refs {
        // A declaration is handled by its exact syntax owner below, not as an import.
        if plan.transfers.iter().any(|t| {
            t.old.declaration.as_ref().is_some_and(|(parent, decl)| {
                *parent == path
                    && rust_syntax::read(parent)
                        .ok()
                        .and_then(|text| crate::signature::offset_of(&text, l, c))
                        == Some(decl.name_at)
            })
        }) {
            continue;
        }
        by_file.entry(path).or_default().push((l, c));
    }
    for (path, mut positions) in by_file {
        // Skipped, its paths would still name the old module (#446).
        let text = std::fs::read_to_string(&path).with_context(|| {
            format!(
                "cannot read {}, where the analyzer reports a path to `{name}`; nothing was \
                 planned",
                path.display()
            )
        })?;
        let owners = plan.graph.owners(&path, None)?;
        let caller = &owners[0];
        let destinations: Vec<_> = plan
            .transfers
            .iter()
            .map(|t| t.destination.clone())
            .collect();
        let new_path = plan.graph.caller_spelling(&owners, &destinations)?;
        let parents: Vec<_> = destinations
            .into_iter()
            .map(|mut owner| {
                owner.module = parent_of(&owner.module);
                owner
            })
            .collect();
        let new_parent_path = plan.graph.caller_spelling(&owners, &parents)?;
        let mut out = text.clone();
        let (mut bare, mut grouped, mut qualified) = (0, 0, 0);
        positions.sort_unstable();
        for (l, c) in positions.into_iter().rev() {
            // A stale position passed over would leave a path to where the module was (#446).
            let site = format!("{}:{l}:{c}", display(root, &path));
            let Some(offset) = crate::signature::offset_of(&out, l, c) else {
                unmatched.push(format!("{site} (the position is not in the file)"));
                continue;
            };
            if !out[offset..].starts_with(name.as_str())
                || out[offset + name.len()..].starts_with(is_ident)
            {
                unmatched.push(format!(
                    "{site} (the analyzer places `{name}` here, but the file says otherwise)"
                ));
                continue;
            }
            if let Some(rewritten) = super::imports::rewrite_reference(
                &out,
                offset,
                &new_path,
                rust_syntax::edition(&caller.root),
            ) {
                out = rewritten;
                qualified += 1;
                continue;
            }
            match spelling(&out, offset) {
                Spelling::Qualified { path_start } => {
                    out.replace_range(path_start..offset, &format!("{new_parent_path}::"));
                    qualified += 1;
                }
                Spelling::Grouped => grouped += 1,
                Spelling::Bare => bare += 1,
            }
        }
        let shown = display(root, &path);
        if qualified > 0 {
            notes.push(format!(
                "{shown}: {qualified} path(s) now through `{new_parent_path}::{name}`"
            ));
        }
        // A grouped import loses the module and imports it on a line of its own.
        if grouped > 0 {
            let (narrowed, dropped) = drop_import(&out, &name);
            out = narrowed;
            for note in dropped {
                notes.push(format!("{shown}: {note}"));
            }
            if !new_parents.contains(&path) {
                out = add_import(&out, &format!("use {new_path};"));
                notes.push(format!("{shown}: added `use {new_path};`"));
            }
        }
        // In the old parent a bare `b` was its own child; now it is imported.
        if bare > 0 && old_parents.contains(&path) {
            out = add_import(&out, &format!("use {new_path};"));
            notes.push(format!("{shown}: added `use {new_path};`"));
        }
        texts.insert(path, out);
    }

    // The declaration leaves the old parent and arrives in the new one.
    super::declarations::transfer(&plan, &mut texts, &name)?;

    // The moved files, at their new paths; `super::` in the module's own file meant the old
    // parent.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut removed = Vec::new();
    for (old, new) in moves {
        let text = match texts.remove(old) {
            Some(t) => t,
            None => std::fs::read_to_string(old)
                .with_context(|| format!("cannot read {}", old.display()))?,
        };
        let text = if old.extension().is_some_and(|ext| ext == "rs") {
            // Undeclared .rs resources are copied byte-for-byte; only known module scopes
            // have a lexical parent whose paths can be rebased.
            let depth = plan
                .graph
                .owners(old, None)
                .ok()
                .map(|owners| owners[0].module.segments.len() - from.segments.len());
            let (resolved, count) = if let Some(depth) = depth {
                rust_syntax::rebase_super(
                    &text,
                    &from.segments,
                    depth,
                    rust_syntax::edition(&plan.transfers[0].old.root),
                )?
            } else {
                (text, 0)
            };
            if count > 0 {
                notes.push(format!(
                    "{}: {count} lexical `super` path(s) preserve their original scope",
                    display(root, new)
                ));
            }
            resolved
        } else {
            text
        };
        rewritten.insert(new.clone(), text);
        removed.push(display(root, old));
    }
    for (path, text) in texts {
        let path = moved_to(&path).unwrap_or(path);
        rewritten.entry(path).or_insert(text);
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let roots: Vec<_> = plan
        .graph
        .nodes
        .iter()
        .filter(|owner| {
            old_parents.contains(&owner.file)
                || new_parents.contains(&owner.file)
                || rewritten.contains_key(&owner.file)
                || moves.iter().any(|(old, _)| old == &owner.file)
        })
        .map(|owner| owner.root.clone())
        .collect();
    for (path, _) in &edits {
        crate::move_item::ownership::filesystem::no_links(root, path)?;
    }
    plan.graph
        .ensure_unaliased(root, edits.iter().map(|(p, _)| p.as_path()))?;
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &roots).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    Ok(ModuleMove {
        module: name,
        root: root.to_path_buf(),
        from_module: from.absolute(),
        to_module: to.absolute(),
        moved: moves
            .iter()
            .map(|(old, new)| (display(root, old), display(root, new)))
            .collect(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        removed,
        notes,
        unmatched,
        diagnostics,
        applied: false,
    })
}
