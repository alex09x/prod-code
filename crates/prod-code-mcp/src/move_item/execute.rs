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

use super::imports::{add_import, carry_imports, drop_import, requalify};
use super::item::{append_item, cut, display, offset_of, span_at, with_doc_comment};
use super::module::{declare_module, module_of, parent_module_file};
use super::types::Move;

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
    let source_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    // A target that does not exist yet is created, and declared by its parent module (#148).
    let new_module_parent = if target.exists() {
        None
    } else {
        anyhow::ensure!(
            target.extension().is_some_and(|e| e == "rs"),
            "{} is not a Rust source file",
            display(root, target)
        );
        Some(parent_module_file(target).with_context(|| {
            format!(
                "{} does not exist, and neither does a module file to declare it in (for \
                 `src/a/x.rs`, `src/a.rs` or `src/a/mod.rs`); create the parent module first",
                display(root, target)
            )
        })?)
    };
    let target_text = match &new_module_parent {
        Some(_) => String::new(),
        None => std::fs::read_to_string(target)
            .with_context(|| format!("cannot read {}", target.display()))?,
    };

    let (_, from_module) = module_of(file)?;
    let (_, to_module) = module_of(target)?;
    anyhow::ensure!(
        from_module != to_module,
        "{} and {} are the same module",
        display(root, file),
        display(root, target)
    );

    let symbols = document_symbols(remote, root, file).await?;
    let (name, decl_start, decl_end) =
        span_at(&symbols, line).with_context(|| format!("no declaration at line {line}"))?;
    // A method belongs to its `impl`; pasted into a module it is a free function with a
    // `self` it cannot have (#196).
    let decl_offset = crate::signature::offset_of(&source_text, decl_start, 1).unwrap_or(0);
    if let Some((owner, _, _, _)) = crate::extract_field::impl_blocks(&source_text)
        .into_iter()
        .find(|(_, _, open, close)| *open < decl_offset && decl_offset < *close)
    {
        anyhow::bail!(
            "`{name}` belongs to `impl {owner}`; move it to another type with \
             `code_move_method` (`prod-code move-method <file> <line> <col>` with `--to-param \
             <name>` for a method, `--to-type <Type>` for an associated function)"
        );
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
    let start = with_doc_comment(&source_text, decl_start);
    let (source_new, item) = cut(&source_text, start, decl_end);
    let (target_with_imports, carried) = carry_imports(&source_text, &item, &target_text);
    let target_new = if target_with_imports.trim().is_empty() {
        format!("{}\n", item.trim())
    } else {
        append_item(&target_with_imports, &item)
    };

    // Ask before the text moves: afterwards the declaration is not where the analyzer left it.
    let refs = crate::signature::references(remote, root, file, line, col)
        .await
        .with_context(|| format!("cannot find the uses of `{name}`; nothing was planned"))?;

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
    let mut left_alone = Vec::new();
    let mut unmatched = Vec::new();
    for (path, positions) in &by_file {
        if path == target {
            continue; // home already
        }
        let Ok((_, module)) = module_of(path) else {
            left_alone.push(format!(
                "{} — outside the ordinary crate layout, its imports were not touched",
                display(root, path)
            ));
            continue;
        };
        let prefix = to_module.spelled_from(&module.krate);
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
            let site = format!("{}:{l}:{c}", display(root, path));
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

    // A new module is declared last: the imports above were placed at the positions the
    // analyzer gave, which a line inserted into the parent first would have shifted.
    let mut created = None;
    if let Some(parent) = &new_module_parent {
        let module_name = target
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let public = item.trim_start().starts_with("pub");
        let parent_text = match rewritten.get(parent) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(parent)
                .with_context(|| format!("cannot read {}", parent.display()))?,
        };
        rewritten.insert(
            parent.clone(),
            declare_module(&parent_text, &module_name, public),
        );
        created = Some((display(root, target), display(root, parent)));
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &[]).await?;
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

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{name}` were not rewritten; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the move does not compile ({} error(s)); nothing was written. Fix the request, or \
             pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Move {
        symbol: name.clone(),
        root: root.to_path_buf(),
        from: display(root, file),
        from_module: from_module.absolute(),
        to: display(root, target),
        to_module: to_module.absolute(),
        new_path: format!("{}::{name}", to_module.absolute()),
        moved_lines: item.lines().count(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        imports,
        left_alone,
        unmatched,
        diagnostics,
        applied,
        created,
    })
}

pub(crate) async fn document_symbols(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Result<serde_json::Value> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await
}
