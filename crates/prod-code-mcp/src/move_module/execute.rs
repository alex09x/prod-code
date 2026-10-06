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

use crate::move_item::{add_import, drop_import, module_of};

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
    anyhow::ensure!(file.is_file(), "{} is not a file", display(root, file));
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let root = &canon(root);
    let file = &canon(file);
    // The target does not exist yet: its directory may not either.
    let target = &match (target.parent(), target.file_name()) {
        (Some(dir), Some(name)) if dir.exists() => canon(dir).join(name),
        _ => target.to_path_buf(),
    };
    anyhow::ensure!(!target.exists(), "{} already exists", display(root, target));
    let (_, from) = module_of(file)?;
    let (_, to) = module_of(target)?;
    let name = from
        .segments
        .last()
        .cloned()
        .context("the crate root is not a module that can move")?;
    anyhow::ensure!(
        to.segments.last() == Some(&name),
        "the module keeps its name `{name}` when it moves; move it, then rename it"
    );
    anyhow::ensure!(from.krate == to.krate, "a module moves within its crate");
    let (from_parent, to_parent) = (parent_of(&from), parent_of(&to));
    anyhow::ensure!(
        from_parent != to_parent,
        "{} is already in {}",
        from.absolute(),
        to_parent.absolute()
    );
    anyhow::ensure!(
        !to_parent.segments.starts_with(&from.segments),
        "a module cannot move into itself"
    );

    let old_parent = declaring_file(file)
        .with_context(|| format!("no module file declares {}", display(root, file)))?;
    let mod_rs = file.file_name().is_some_and(|n| n == "mod.rs");
    let new_file = if mod_rs {
        target.with_extension("").join("mod.rs")
    } else {
        target.to_path_buf()
    };
    let new_parent = declaring_file(&new_file).with_context(|| {
        format!(
            "{} has no parent module file to declare it in (for `src/c/b.rs`, `src/c.rs` or \
             `src/c/mod.rs`); create the parent module first",
            display(root, target)
        )
    })?;
    // The submodules live in the module's directory: `src/a/b/` for both layouts.
    let old_dir = if mod_rs {
        file.parent().map(Path::to_path_buf)
    } else {
        Some(file.with_extension(""))
    }
    .filter(|d| d.is_dir());
    let new_dir = if mod_rs {
        new_file.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        new_file.with_extension("")
    };
    if old_dir.is_some() {
        anyhow::ensure!(
            !new_dir.exists(),
            "{} already exists",
            display(root, &new_dir)
        );
    }

    let parent_text = std::fs::read_to_string(&old_parent)
        .with_context(|| format!("cannot read {}", old_parent.display()))?;
    let (decl_start, decl_end, decl_line) = declaration(&parent_text, &name).with_context(|| {
        format!(
            "{} does not declare `mod {name};` at its top level (an inline `mod {name} {{ … }}` \
             or a `#[path]` module is not moved)",
            display(root, &old_parent)
        )
    })?;
    let block = parent_text[decl_start..decl_end].to_string();
    anyhow::ensure!(
        !block.contains("#[path"),
        "`{name}` is declared with `#[path]`; its file is not where the layout puts it"
    );
    let new_parent_text = std::fs::read_to_string(&new_parent)
        .with_context(|| format!("cannot read {}", new_parent.display()))?;
    anyhow::ensure!(
        declaration(&new_parent_text, &name).is_none(),
        "{} already declares a module `{name}`",
        display(root, &new_parent)
    );

    // Where the analyzer says the module is named, asked before any text moves.
    let line_start = parent_text[..decl_end.saturating_sub(1)]
        .rfind('\n')
        .map_or(0, |i| i + 1);
    let decl_line_no = parent_text[..line_start].matches('\n').count() as u32 + 1;
    let name_col = decl_line.rfind(&format!("mod {name};")).unwrap_or(0) as u32 + 5;
    let refs = crate::signature::references(remote, root, &old_parent, decl_line_no, name_col)
        .await
        .with_context(|| format!("cannot find the paths to `{name}`; nothing was planned"))?;

    // The files that move: the module's own, and everything in its directory.
    let mut moves: Vec<(PathBuf, PathBuf)> = vec![(file.to_path_buf(), new_file.clone())];
    if let Some(dir) = &old_dir {
        for f in files_under(dir) {
            if f == *file {
                continue;
            }
            if let Ok(rel) = f.strip_prefix(dir) {
                moves.push((f.clone(), new_dir.join(rel)));
            }
        }
    }
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
        by_file.entry(canon(&path)).or_default().push((l, c));
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
        let Ok((_, module)) = module_of(&path) else {
            notes.push(format!(
                "{} — outside the ordinary crate layout, not rewritten",
                display(root, &path)
            ));
            continue;
        };
        let new_parent_path = to_parent.spelled_from(&module.krate);
        let new_path = to.spelled_from(&module.krate);
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
            if path != new_parent {
                out = add_import(&out, &format!("use {new_path};"));
                notes.push(format!("{shown}: added `use {new_path};`"));
            }
        }
        // In the old parent a bare `b` was its own child; now it is imported.
        if bare > 0 && path == old_parent {
            out = add_import(&out, &format!("use {new_path};"));
            notes.push(format!("{shown}: added `use {new_path};`"));
        }
        texts.insert(path, out);
    }

    // The declaration leaves the old parent and arrives in the new one.
    let old_parent_now = texts
        .get(&old_parent)
        .cloned()
        .unwrap_or_else(|| parent_text.clone());
    let (start, end, _) = declaration(&old_parent_now, &name)
        .context("the declaration was rewritten while its paths were")?;
    let mut old_parent_new = old_parent_now.clone();
    old_parent_new.replace_range(start..end, "");
    texts.insert(old_parent.clone(), old_parent_new);
    let new_parent_now = texts.get(&new_parent).cloned().unwrap_or(new_parent_text);
    // An import of the module in its new parent would now clash with the declaration.
    let (new_parent_now, dropped) = drop_import(&new_parent_now, &name);
    for note in dropped {
        notes.push(format!("{}: {note}", display(root, &new_parent)));
    }
    texts.insert(
        new_parent.clone(),
        declare_block(&new_parent_now, &name, &block),
    );

    // The moved files, at their new paths; `super::` in the module's own file meant the old
    // parent.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut removed = Vec::new();
    for (old, new) in &moves {
        let text = match texts.remove(old) {
            Some(t) => t,
            None => std::fs::read_to_string(old)
                .with_context(|| format!("cannot read {}", old.display()))?,
        };
        let text = if old == file {
            let head = from_parent.spelled_from(&from.krate);
            let (resolved, count) = resolve_super(&text, &head);
            if count > 0 {
                notes.push(format!(
                    "{}: {count} `super::` now `{head}::`",
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
