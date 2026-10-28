/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::budget::{MoveBudget, document_symbols};
use super::child::move_to_child;
use super::imports::{add_import, carry_imports, drop_import, has_use_super_glob, requalify};
use super::item::{append_item, cut, display, move_inherent_method, offset_of, span_at};
use super::module::extract_inline_module;
use super::types::Move;
use super::{ownership::filesystem, plan::ItemPlan, rust_syntax};

/// Moves the declaration at `file:line:col` into `target`, rewriting the imports of every file
/// that uses it. Nothing is written unless `apply`, and not then if it does not compile.
#[allow(clippy::too_many_arguments)]
pub async fn move_item(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    target: &Path,
    apply: bool,
    force: bool,
) -> Result<Move> {
    anyhow::ensure!(
        file != target,
        "the item is already in {}",
        display(root, target)
    );
    let ext = file.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext != "rs" {
        return crate::move_polyglot::move_item(
            remote, root, file, line, col, target, apply, force,
        )
        .await;
    }
    let budget = MoveBudget::new();
    anyhow::ensure!(
        target.extension().is_some_and(|e| e == "rs"),
        "{} is not a Rust source file",
        display(root, target)
    );
    filesystem::no_links(root, file)?;
    filesystem::no_links(root, target)?;
    let source_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let plan = ItemPlan::new(root, file, target, line, col, &source_text)?;
    let (graph, from_owners, to_owners) = (&plan.graph, &plan.from, &plan.to);
    let from_module = from_owners[0].module.clone();
    let mut to_module = to_owners[0].module.clone();
    let mut new_module_parent = plan.new_module_parent.clone();
    let mut target_text = plan.target_text.clone();
    let edition = rust_syntax::edition(&from_owners[0].root);
    anyhow::ensure!(
        from_module != to_module,
        "{} and {} are the same module",
        display(root, file),
        display(root, target)
    );

    let symbols = budget
        .run(
            "reading document symbols",
            document_symbols(remote, root, file),
        )
        .await?;
    let child = from_module.krate == to_module.krate
        && to_module.segments.len() == from_module.segments.len() + 1
        && to_module.segments.starts_with(&from_module.segments);
    let (name, decl_start, decl_end) =
        span_at(&symbols, line).with_context(|| format!("no declaration at line {line}"))?;
    // A method belongs to its `impl`; pasted into a module without a verified inherent `impl` of
    // the same resolved owner type it cannot be moved by `code_move` (#196, #1084).
    let decl_offset = crate::signature::offset_of(&source_text, decl_start, 1).unwrap_or(0);
    let enclosing_impl = crate::extract_field::impl_blocks(&source_text)
        .into_iter()
        .find(|(_, _, open, close)| *open < decl_offset && decl_offset < *close);
    let mut target_impl_idx = None;
    let mut target_owner_binding = None;
    if let Some((ref owner, impl_at, impl_open, _)) = enclosing_impl {
        let is_inherent = !source_text[impl_at..impl_open].contains(" for ");
        if is_inherent && from_module.krate == to_module.krate {
            let plan = super::target_impl::prepare_target_impl(
                &budget,
                remote,
                root,
                file,
                &source_text,
                (impl_at, impl_open),
                target,
                &target_text,
                owner,
                new_module_parent.is_some(),
                &graph,
                &to_owners,
                edition,
            )
            .await?;
            target_text = plan.text;
            target_impl_idx = Some(plan.index);
            target_owner_binding = Some(plan.owner_binding);
        }
        if target_impl_idx.is_none() {
            anyhow::bail!(
                "`{name}` belongs to `impl {owner}`; move it to another type with \
                 `code_move_method` (`prod-code move-method <file> <line> <col>` with `--to-param \
                 <name>` for a method, `--to-type <Type>` for an associated function)"
            );
        }
    }
    // A `mod x;` line declares a module whose code is in its own file; cutting the line would
    // move nothing of it (#188).
    let module_declaration = format!("mod {name};");
    if source_text
        .lines()
        .skip(decl_start.saturating_sub(1) as usize)
        .take((decl_end + 1).saturating_sub(decl_start) as usize)
        .any(|l| l.trim_end().ends_with(&module_declaration))
    {
        anyhow::bail!(
            "`{name}` is a module with its own file; move the module with `code_move_module` \
             (`prod-code move-module <its file> --to <new file>`)"
        );
    }
    rust_syntax::ensure_whole_lines(&source_text, decl_start, &name, edition)?;
    let start = rust_syntax::item_start(&source_text, decl_start, edition);
    let mut created = None;
    let mut extracted_inline_mod = false;
    let (source_new, mut target_new, item, mut carried) =
        if let Some((_, impl_at, impl_open, impl_close)) = enclosing_impl {
            move_inherent_method(
                &source_text,
                &target_text,
                target_impl_idx,
                (impl_at, impl_open, impl_close),
                start,
                decl_start,
                decl_end,
            )
        } else if target_text.trim().is_empty()
            && let Some((source_new, target_new)) =
                extract_inline_module(&source_text, start, decl_end, &name, file, target)
        {
            new_module_parent = None;
            extracted_inline_mod = true;
            let mut segs = from_module.segments.clone();
            segs.push(name.clone());
            to_module.segments = segs;
            created = Some((display(root, target), display(root, file)));
            let (_, raw_item) = cut(&source_text, start, decl_end);
            (source_new, target_new, raw_item, Vec::new())
        } else {
            let (source_new, item) = cut(&source_text, start, decl_end);
            let (target_with_imports, item, carried) = if child {
                move_to_child(
                    &source_text,
                    &item,
                    &target_text,
                    from_owners[0].test_crate,
                    edition,
                )?
            } else if to_owners.iter().any(|owner| {
                owner
                    .declaration
                    .as_ref()
                    .is_some_and(|(parent, _)| parent == file)
            }) && has_use_super_glob(&target_text)
            {
                (target_text.clone(), item, Vec::new())
            } else {
                let (text, carried) = carry_imports(&source_text, &item, &target_text);
                (text, item, carried)
            };
            let target_new = if target_with_imports.trim().is_empty() {
                let ending = source_text
                    .split_inclusive('\n')
                    .nth(decl_end.saturating_sub(1) as usize)
                    .filter(|line| line.ends_with("\r\n"))
                    .map_or("\n", |_| "\r\n");
                format!("{item}{ending}")
            } else {
                append_item(&target_with_imports, &item)
            };
            (source_new, target_new, item, carried)
        };
    if let Some(binding) = target_owner_binding {
        let (text, note) =
            super::target_impl::ensure_owner_binding(&target_new, &item, &binding, edition)?;
        target_new = text;
        if let Some(note) = note {
            carried.push(note);
        }
    }

    // Ask before the text moves: afterwards the declaration is not where the analyzer left it.
    let refs = if extracted_inline_mod {
        Vec::new()
    } else {
        budget
            .run(
                "finding references",
                crate::signature::references(remote, root, file, line, col),
            )
            .await
            .with_context(|| format!("cannot find the uses of `{name}`; nothing was planned"))?
    };

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), source_new);
    rewritten.insert(target.to_path_buf(), target_new);
    // The analyzer reported these against the file as it is now. In the file the item just
    // left, everything below the hole has moved up by as many lines as the item was long, and
    // a position that is not adjusted points at the wrong text.
    let removed = decl_end - start + 1;
    let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32)>> = BTreeMap::new();
    for (path, l, c) in refs {
        let l = if path == file {
            // A reference inside the item travelled with it.
            if l >= start && l <= decl_end {
                continue;
            }
            if l > decl_end { l - removed } else { l }
        } else {
            l
        };
        by_file.entry(path).or_default().push((l, c));
    }

    let mut imports: Vec<String> = carried
        .into_iter()
        .map(|note| format!("{}: {note}", display(root, target)))
        .collect();
    let left_alone = Vec::new();
    let mut unmatched = Vec::new();
    let mut callers: Vec<PathBuf> = Vec::new();
    if enclosing_impl.is_some() {
        for path in by_file.keys() {
            if !rewritten.contains_key(path) {
                callers.push(path.clone());
            }
        }
    } else {
        for (path, positions) in &by_file {
            if path == target {
                // The declaration now lives here, but an import of its old path (notably a
                // parent's re-export from the child) still points at the file it left (#1201).
                let (text, dropped) = drop_import(&rewritten[target], &name);
                for note in dropped {
                    imports.push(format!("{}: {note}", display(root, path)));
                }
                rewritten.insert(target.to_path_buf(), text);
                continue;
            }
            let callers = graph.owners(path, None)?;
            let prefix = graph.caller_spelling(&callers, to_owners)?;
            // Taken as empty, the file would be written back as only its new imports (#446).
            let current = match rewritten.get(path) {
                Some(text) => text.clone(),
                None => std::fs::read_to_string(path).with_context(|| {
                    format!(
                        "cannot read {}, where the analyzer reports a use of `{name}`; nothing was \
                         planned",
                        path.display()
                    )
                })?,
            };
            // `requalify` passes over a position that does not name the item; such a use would
            // still reach it where it was (#446). Its edits end at each position, so the text it
            // sees there is this one.
            for (l, c) in positions {
                // Diagnostics name the original on-disk position, not the line after cutting
                // the declaration out of the proposed text.
                let original_line = if path == file && *l >= start {
                    *l + removed
                } else {
                    *l
                };
                let site = format!("{}:{original_line}:{c}", display(root, path));
                match offset_of(&current, *l, *c) {
                    None => unmatched.push(format!("{site} (the position is not in the file)")),
                    Some(at)
                        if !current[at..].starts_with(name.as_str())
                            || current[at + name.len()..]
                                .starts_with(|c: char| c.is_alphanumeric() || c == '_') =>
                    {
                        unmatched.push(format!(
                            "{site} (the analyzer places `{name}` here, but the file says otherwise)"
                        ))
                    }
                    Some(_) => {}
                }
            }
            let (requalified, bare) = requalify(&current, positions, &name, &prefix);
            let (mut text, dropped) = drop_import(&requalified, &name);
            for note in dropped {
                imports.push(format!("{}: {note}", display(root, path)));
            }
            // Only a file that still spells the name on its own needs to import it; one where
            // every use was path-qualified now names the new module inline and needs nothing.
            if bare > 0 {
                let use_line = format!("use {prefix}::{name};");
                text = add_import(&text, &use_line);
                imports.push(format!("{}: added `{use_line}`", display(root, path)));
            }
            rewritten.insert(path.clone(), text);
        }
    }

    // A new module is declared last: the imports above were placed at the positions the
    // analyzer gave, which a line inserted into the parent first would have shifted.
    if let Some(parent) = &new_module_parent {
        let public = rust_syntax::public(&source_text, decl_start, edition);
        plan.declare(&mut rewritten, target, public)?;
        created = Some((display(root, target), display(root, parent)));
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    callers.extend(from_owners.iter().map(|owner| owner.root.clone()));
    callers.extend(
        graph
            .nodes
            .iter()
            .filter(|owner| rewritten.contains_key(&owner.file))
            .map(|owner| owner.root.clone()),
    );
    callers.sort();
    callers.dedup();
    for (path, _) in &edits {
        filesystem::no_links(root, path)?;
    }
    graph.ensure_unaliased(root, edits.iter().map(|(p, _)| p.as_path()))?;
    let diagnostics = budget.validate(remote, root, &edits, &callers).await?;

    let mut result = Move {
        symbol: name.clone(),
        root: root.to_path_buf(),
        from: display(root, file),
        from_module: from_module.absolute(),
        to: display(root, target),
        to_module: to_module.absolute(),
        new_path: if extracted_inline_mod {
            to_module.absolute()
        } else {
            format!("{}::{name}", to_module.absolute())
        },
        moved_lines: item.lines().count(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        imports,
        left_alone,
        unmatched,
        diagnostics,
        applied: false,
        created,
    };
    if apply {
        result.write(force)?;
    }
    Ok(result)
}
