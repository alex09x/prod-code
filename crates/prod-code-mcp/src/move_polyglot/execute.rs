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

use crate::move_item::Move;
use crate::parameter_object::Language;

use super::callers::rewrite_caller_imports;
use super::decl::{find_polyglot_decl, with_doc_comment_polyglot};
use super::go_imports::remove_unused_go_imports;
use super::imports::{carry_imports_polyglot, update_source_imports};
use super::specifiers::{display, is_compatible_language_family, module_display_name};
use super::target::{
    check_target_collision, cpp_move_target_is_implementation, format_item_for_target,
    initial_file_header,
};

#[allow(clippy::too_many_arguments)]
pub async fn move_item(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    _col: u32,
    target: &Path,
    apply: bool,
    force: bool,
) -> Result<Move> {
    anyhow::ensure!(
        file != target,
        "the item is already in {}",
        display(root, target)
    );

    let lang = Language::of(file)
        .with_context(|| format!("unsupported source language for {}", file.display()))?;
    let target_lang = Language::of(target)
        .with_context(|| format!("unsupported target language for {}", target.display()))?;

    anyhow::ensure!(
        is_compatible_language_family(lang, target_lang),
        "cannot move between incompatible languages ({} and {})",
        lang.fence(),
        target_lang.fence()
    );
    if cpp_move_target_is_implementation(lang, target) {
        anyhow::bail!(
            "cannot move a C/C++ declaration into an implementation file: callers cannot safely include a `.cpp`/`.c` file; move it to a header instead"
        );
    }

    let source_text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let target_existed = target.exists();
    let target_text = if target_existed {
        std::fs::read_to_string(target)
            .with_context(|| format!("cannot read {}", target.display()))?
    } else {
        initial_file_header(target, target_lang)
    };

    let (name, decl_start, decl_end) = if let Ok(symbols) =
        crate::move_item::document_symbols(remote, root, file).await
        && let Some((s_name, s, e)) = crate::move_item::span_at(&symbols, line)
        && !s_name.is_empty()
    {
        (s_name, s, e)
    } else {
        find_polyglot_decl(&source_text, line, lang)?
    };

    if target_existed {
        check_target_collision(&target_text, &name, target_lang)?;
    }

    let start = with_doc_comment_polyglot(&source_text, decl_start, lang);
    let (source_new_cut, item_raw) = crate::move_item::cut(&source_text, start, decl_end);
    let source_new_cut = if lang == Language::Go {
        remove_unused_go_imports(&source_new_cut)
    } else {
        source_new_cut
    };
    let item = format_item_for_target(&item_raw, lang);

    let (target_with_carried, carried_notes) =
        carry_imports_polyglot(&source_text, &item, &target_text, file, target, lang, root);

    let target_new = if target_with_carried.trim().is_empty() {
        format!("{}\n", item.trim())
    } else {
        crate::move_item::append_item(&target_with_carried, &item)
    };

    let (source_new, source_import_note) =
        update_source_imports(&source_new_cut, &name, file, target, root, lang)?;

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), source_new);
    rewritten.insert(target.to_path_buf(), target_new);

    let mut import_notes = Vec::new();
    for note in carried_notes {
        import_notes.push(format!("{}: {note}", display(root, target)));
    }
    if let Some(note) = source_import_note {
        import_notes.push(format!("{}: {note}", display(root, file)));
    }

    let workspace_sources = crate::signature_polyglot::collect_workspace_sources(root, lang);
    for src in workspace_sources {
        if src == file || src == target {
            continue;
        }
        let Ok(caller_text) = std::fs::read_to_string(&src) else {
            continue;
        };
        if let Some((rewritten_caller, note)) =
            rewrite_caller_imports(&caller_text, &name, &src, file, target, root, lang)
        {
            import_notes.push(format!("{}: {note}", display(root, &src)));
            rewritten.insert(src, rewritten_caller);
        }
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
            diagnostics.is_empty() || force,
            "the move does not compile ({} error(s)); nothing was written. Fix the request, or              pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    let from_mod = module_display_name(file, root, lang);
    let to_mod = module_display_name(target, root, lang);
    let new_path = match lang {
        Language::Python => format!("{to_mod}.{name}"),
        Language::Go => format!("{to_mod}.{name}"),
        Language::Swift => name.clone(),
        _ => format!("{to_mod}::{name}"),
    };

    let created = if target_existed {
        None
    } else {
        Some((
            display(root, target),
            display(root, target.parent().unwrap_or(root)),
        ))
    };

    Ok(Move {
        symbol: name,
        root: root.to_path_buf(),
        from: display(root, file),
        from_module: from_mod,
        to: display(root, target),
        to_module: to_mod,
        new_path,
        moved_lines: (decl_end - start + 1) as usize,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        imports: import_notes,
        left_alone: Vec::new(),
        unmatched: Vec::new(),
        diagnostics,
        applied,
        created,
    })
}
