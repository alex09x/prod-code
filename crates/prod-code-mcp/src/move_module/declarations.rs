/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{plan::Plan, syntax::declare_block_in};
use crate::move_item::rust_syntax as syntax;
use anyhow::{Context, Result};
use ra_ap_syntax::{AstNode, ast};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) fn transfer(
    plan: &Plan,
    texts: &mut BTreeMap<PathBuf, String>,
    name: &str,
) -> Result<()> {
    let mut removals: BTreeMap<PathBuf, Vec<_>> = BTreeMap::new();
    let mut blocks: BTreeMap<PathBuf, String> = BTreeMap::new();
    for transfer in &plan.transfers {
        let old = plan.graph.parent(&transfer.old)?;
        let text = texts
            .entry(old.file.clone())
            .or_insert(syntax::read(&old.file)?);
        let scope = plan.graph.scope_in(&old, text)?;
        let edition = syntax::edition(&old.root);
        let decl = syntax::declarations(text, scope, edition)
            .into_iter()
            .find(|d| d.name == name && d.inline.is_none())
            .context("source declaration changed; nothing was written")?;
        let start = syntax::line_start(text, decl.span.start);
        let start = if text[start..decl.span.start].trim().is_empty() {
            start
        } else {
            decl.span.start
        };
        let end = syntax::line_end(text, decl.span.end);
        let block = retarget(
            &text[start..end],
            transfer.parent.attribute_base(),
            &transfer.destination.file,
            edition,
            transfer.destination.file != transfer.parent.module_dir.join(format!("{name}.rs"))
                && transfer.destination.file
                    != transfer.parent.module_dir.join(name).join("mod.rs"),
        )?;
        // Shared physical parents get a single declaration. Root visibility may differ;
        // the public declaration preserves accessibility through a public library root.
        if let Some(previous) = blocks.get(&transfer.parent.file) {
            let (old_prefix, old_visibility) = metadata(previous, edition)?;
            let (prefix, visibility) = metadata(&block, edition)?;
            anyhow::ensure!(
                old_prefix == prefix,
                "shared destination cannot preserve different per-root module attributes/docs; nothing was written"
            );
            let mut combined = previous.clone();
            if old_visibility != visibility {
                anyhow::ensure!(
                    old_visibility == "pub" || visibility == "pub",
                    "shared destination has incompatible restricted visibility; nothing was written"
                );
                if visibility == "pub" {
                    combined = block.clone();
                }
            }
            // Per-root path attributes can differ at the source, but resolve to the same
            // physical destination here. Retain a path attribute when any owner supplied it.
            if path_attribute(&combined, edition).is_none()
                && let Some(attr) =
                    path_attribute(previous, edition).or_else(|| path_attribute(&block, edition))
            {
                combined.insert_str(0, &format!("{attr}\n"));
            }
            blocks.insert(transfer.parent.file.clone(), combined);
        } else {
            blocks.insert(transfer.parent.file.clone(), block);
        }
        removals.entry(old.file).or_default().push(start..end);
    }
    for (file, mut spans) in removals {
        spans.sort_by_key(|r| r.start);
        spans.dedup();
        let text = texts.get_mut(&file).unwrap();
        for span in spans.into_iter().rev() {
            text.replace_range(span, "");
        }
    }
    let mut inserted = BTreeSet::new();
    for transfer in &plan.transfers {
        let parent = &transfer.parent;
        if !inserted.insert(parent.file.clone()) {
            continue;
        }
        let text = texts
            .entry(parent.file.clone())
            .or_insert(syntax::read(&parent.file)?);
        let edition = syntax::edition(&parent.root);
        let scope = plan.graph.scope_in(parent, text)?;
        *text = super::imports::remove_arriving_imports(
            text,
            scope,
            &parent.module.segments,
            &transfer.old.module.segments,
            &transfer.destination.module.segments,
            edition,
        )?;
        let scope = plan.graph.scope_in(parent, text)?;
        let existing = syntax::declarations(text, scope.clone(), edition)
            .into_iter()
            .find(|d| d.name == name);
        let block = &blocks[&parent.file];
        if let Some(decl) = existing {
            // An exact pre-existing missing-file declaration owns this target. Its syntax
            // (visibility, attributes, path) stays; source attributes/docs travel with it.
            anyhow::ensure!(
                transfer
                    .destination
                    .declaration
                    .as_ref()
                    .is_some_and(|(_, d)| d.name == decl.name),
                "destination already declares another module with this name; nothing was written"
            );
            let tree = syntax::parse(block, edition);
            let module = tree
                .syntax()
                .children()
                .find_map(ast::Module::cast)
                .context("moved declaration is not a module")?;
            let prefix_end = module
                .syntax()
                .children()
                .find_map(ast::Visibility::cast)
                .map(|v| syntax::range(v.syntax()).start)
                .or_else(|| {
                    module
                        .syntax()
                        .children_with_tokens()
                        .filter_map(|n| n.into_token())
                        .find(|t| t.text() == "mod")
                        .map(|t| usize::from(t.text_range().start()))
                })
                .unwrap_or(0);
            let mut prefix = block[..prefix_end].to_string();
            let path_spans: Vec<_> = module
                .syntax()
                .children()
                .filter_map(ast::Attr::cast)
                .filter(|a| a.simple_name().as_deref() == Some("path"))
                .map(|a| syntax::range(a.syntax()))
                .filter(|r| r.end <= prefix.len())
                .collect();
            for r in path_spans.into_iter().rev() {
                let end = syntax::line_end(&prefix, r.end);
                prefix.replace_range(r.start..end, "");
            }
            let require_path = plan.transfers.iter().any(|t| {
                t.parent.file == parent.file
                    && t.destination.file != t.parent.module_dir.join(format!("{name}.rs"))
                    && t.destination.file != t.parent.module_dir.join(name).join("mod.rs")
            });
            let updated = retarget(
                &text[decl.span.clone()],
                parent.attribute_base(),
                &transfer.destination.file,
                edition,
                require_path,
            )?;
            text.replace_range(decl.span, &format!("{prefix}{updated}"));
        } else if parent.scope.start == 0 {
            *text = declare_block_in(text, name, block, edition);
        } else {
            text.insert_str(scope.end - 1, &format!("\n{block}\n"));
        }
    }
    Ok(())
}

fn metadata(block: &str, edition: ra_ap_syntax::Edition) -> Result<(String, String)> {
    let tree = syntax::parse(block, edition);
    let module = tree
        .syntax()
        .children()
        .find_map(ast::Module::cast)
        .context("moved declaration is not a module")?;
    let visibility = module.syntax().children().find_map(ast::Visibility::cast);
    let mut ignored: Vec<_> = module
        .syntax()
        .children()
        .filter_map(ast::Attr::cast)
        .filter(|a| a.simple_name().as_deref() == Some("path"))
        .map(|a| syntax::range(a.syntax()))
        .collect();
    if let Some(vis) = &visibility {
        ignored.push(syntax::range(vis.syntax()));
    }
    let fingerprint: Vec<_> = module
        .syntax()
        .descendants_with_tokens()
        .filter_map(|n| n.into_token())
        .filter(|t| {
            !t.text().trim().is_empty()
                && !ignored.iter().any(|r| {
                    r.start <= usize::from(t.text_range().start())
                        && r.end >= usize::from(t.text_range().end())
                })
        })
        .map(|t| t.text().to_string())
        .collect();
    Ok((
        fingerprint.join("\0"),
        visibility.map_or_else(String::new, |v| v.syntax().text().to_string()),
    ))
}

fn path_attribute(block: &str, edition: ra_ap_syntax::Edition) -> Option<String> {
    syntax::parse(block, edition)
        .syntax()
        .children()
        .flat_map(|n| n.children())
        .filter_map(ast::Attr::cast)
        .find(|a| a.simple_name().as_deref() == Some("path"))
        .map(|a| a.syntax().text().to_string())
}

fn retarget(
    block: &str,
    base: &Path,
    target: &Path,
    edition: ra_ap_syntax::Edition,
    require_path: bool,
) -> Result<String> {
    let tree = syntax::parse(block, edition);
    let mut out = block.to_string();
    let literal = crate::move_item::ownership::filesystem::path_literal(base, target)?;
    let mut spans: Vec<_> = tree
        .syntax()
        .children()
        .flat_map(|n| n.children())
        .filter_map(ast::Attr::cast)
        .flat_map(syntax::active_path_literals)
        .collect();
    let has_path = !spans.is_empty();
    spans.sort_by_key(|r| r.start);
    for span in spans.into_iter().rev() {
        out.replace_range(span, &literal);
    }
    if require_path && !has_path {
        out.insert_str(0, &format!("#[path={literal}]\n"));
    }
    Ok(out)
}
