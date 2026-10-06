/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::call_matching::sync_cpp_headers;
use super::sites::{collect_sites, convert_sites, display, mark_tried};
use super::spans::{declared_type_span_polyglot, find_symbol_decl_offset};
use super::transitive::propagate_transitive;
use super::types::{Converted, Migration};
use crate::parameter_object::Language;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Changes the declared type of the symbol at `file:line:col` and reports what no longer fits.
#[allow(clippy::too_many_arguments)]
pub async fn migrate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    to: &str,
    convert: bool,
    apply: bool,
    force: bool,
) -> Result<Migration> {
    migrate_ext(
        remote,
        root,
        file,
        None,
        Some(line),
        Some(col),
        to,
        convert,
        false,
        apply,
        force,
    )
    .await
}

/// Polyglot, transitive type migration across Rust, TypeScript, JavaScript, Python, C++, Swift, and Go.
#[allow(clippy::too_many_arguments)]
pub async fn migrate_ext(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    to: &str,
    convert: bool,
    transitive: bool,
    apply: bool,
    force: bool,
) -> Result<Migration> {
    anyhow::ensure!(!to.trim().is_empty(), "the new type is empty");
    let lang = Language::of(file).unwrap_or(Language::Rust);
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;

    let (offset, name) = if let (Some(l), Some(c)) = (line, col) {
        let off = crate::signature::offset_of(&text, l, c)
            .context("the declaration is not at the resolved position")?;
        let n: String = text[off..]
            .chars()
            .take_while(|ch| ch.is_alphanumeric() || *ch == '_')
            .collect();
        anyhow::ensure!(!n.is_empty(), "there is no declared name at that position");
        (off, n)
    } else if let Some(sym) = symbol {
        let clean_sym = sym
            .rsplit("::")
            .next()
            .unwrap_or(sym)
            .rsplit('.')
            .next()
            .unwrap_or(sym)
            .trim();
        let off = find_symbol_decl_offset(&text, clean_sym, lang, line).with_context(|| {
            format!(
                "could not locate declaration of `{clean_sym}` in {}",
                file.display()
            )
        })?;
        (off, clean_sym.to_string())
    } else {
        anyhow::bail!("Missing 'symbol' or 'line' and 'character'");
    };

    let (start, end) = declared_type_span_polyglot(&text, offset, lang).with_context(|| {
        format!(
            "`{name}` has no declared type this understands: a field, a parameter, an annotated \
             variable or a function's return type"
        )
    })?;
    let was = text[start..end].trim().to_string();
    anyhow::ensure!(was != to.trim(), "`{name}` is already declared as `{to}`");

    let mut new_text = text.clone();
    new_text.replace_range(start..end, to);
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    rewritten.insert(file.to_path_buf(), new_text);

    let mut also: Vec<PathBuf> = if let (Some(l), Some(c)) = (line, col) {
        crate::signature::references(remote, root, file, l, c)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(path, _, _)| path)
            .filter(|path| path != file)
            .collect()
    } else {
        Vec::new()
    };
    also.sort();
    also.dedup();

    // C/C++ prototype synchronization in headers
    if matches!(lang, Language::Cpp | Language::C) {
        sync_cpp_headers(
            root,
            file,
            &name,
            &was,
            to,
            &mut rewritten,
            &mut also,
            false,
        );
    }

    let mut transitive_count = 0;
    let mut transitively_migrated = Vec::new();
    if transitive {
        let (t_count, t_migrated) =
            propagate_transitive(root, &mut rewritten, file, &name, &was, to, lang, &mut also);
        transitive_count = t_count;
        transitively_migrated = t_migrated;
    }

    let edits: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &edits, &also).await?;

    let (mut sites, mut in_attributes) = collect_sites(root, &rewritten, &reports, &was, to);

    let mut converted = Vec::new();
    let mut conversion_note = None;
    if convert {
        let outcome = convert_sites(remote, root, &rewritten, &also, &sites, &was, to).await?;
        match outcome {
            Converted::Accepted {
                texts,
                conversions,
                reports,
                tried,
            } => {
                let (after, attrs) = collect_sites(root, &texts, &reports, &was, to);
                sites = after;
                in_attributes = attrs;
                rewritten = texts;
                converted = conversions;
                mark_tried(&mut sites, &tried);
            }
            Converted::Nothing { tried } => mark_tried(&mut sites, &tried),
            Converted::Dropped { note, tried } => {
                conversion_note = Some(note);
                mark_tried(&mut sites, &tried);
            }
        }
    }

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            sites.is_empty() || force,
            "{} site(s) do not fit the new type; nothing was written. Read them first, then \
             pass `force: true` to write the declaration{} and migrate the sites yourself",
            sites.len(),
            if converted.is_empty() {
                ""
            } else {
                " and the conversions"
            }
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(Migration {
        symbol: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now: to.to_string(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        sites,
        in_attributes,
        converted,
        conversion_note,
        applied,
        transitive_count,
        transitively_migrated,
    })
}
