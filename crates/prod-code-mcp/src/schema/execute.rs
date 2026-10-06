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

use super::casing::variants;
use super::detect::{Kind, kind_of, label, schema_of};
use super::edits::{
    NOT_FOUND, in_comment, overlaps, ranged_edits, rename_symbol, span_of, still_spelled,
    write_rewritten,
};
use super::scan::{display, edit_for, lsp_position, scan, text_rewrite_decision, walk};
use super::types::{Occurrence, SchemaRename, lsp_end_position};

pub async fn rename(
    remote: SocketAddr,
    root: &Path,
    field: &str,
    to: &str,
    apply: bool,
    force: bool,
    scope: Option<&Path>,
) -> Result<SchemaRename> {
    anyhow::ensure!(
        field.chars().count() >= 3 || force,
        "`{field}` is too short to look for safely; pass `force: true` if you mean it"
    );
    anyhow::ensure!(field != to, "`{field}` and `{to}` are the same name");
    let variants = variants(field, to);
    anyhow::ensure!(
        !variants.is_empty(),
        "`{field}` has no spellings to look for"
    );

    let area = scope.unwrap_or(root);
    const MAX_FILE: u64 = 512 * 1024;
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut found: Vec<Occurrence> = Vec::new();
    for file in walk(area, MAX_FILE) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue; // not UTF-8: not a source file
        };
        let hits = scan(&text, &variants, &file);
        if !hits.is_empty() {
            found.extend(hits);
            originals.insert(file, text);
        }
    }
    anyhow::ensure!(
        !found.is_empty(),
        "`{field}` {NOT_FOUND} {}",
        match display(root, area).as_str() {
            "" => "this workspace".to_string(),
            rel => rel.to_string(),
        }
    );
    const MAX_OCCURRENCES: usize = 400;
    anyhow::ensure!(
        found.len() <= MAX_OCCURRENCES || force,
        "{} occurrences of `{field}` — too many to rewrite in one step; narrow it with `path`, \
         or pass `force: true`",
        found.len()
    );

    // Phase one: the analyzers.
    const MAX_RENAMES: usize = 60;
    let mut whole: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut ranged: BTreeMap<PathBuf, Vec<serde_json::Value>> = BTreeMap::new();
    let mut claimed: BTreeMap<PathBuf, Vec<(u32, u32, u32, u32)>> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    let mut tried: Vec<(PathBuf, u32, u32)> = Vec::new();
    // Occurrences a rename already covered. Without this the loop asks the analyzer about
    // every place it has just rewritten, and every answer looks like a collision with the
    // answer that rewrote it.
    let mut done: Vec<(PathBuf, u32, u32)> = Vec::new();
    for _ in 0..MAX_RENAMES {
        let Some(o) = found
            .iter()
            .find(|o| {
                matches!(kind_of(&o.file), Kind::Code(_))
                    && !o.in_string
                    && !in_comment(&originals, o)
                    && !tried.contains(&(o.file.clone(), o.line, o.col))
                    && !done.contains(&(o.file.clone(), o.line, o.col))
            })
            .cloned()
        else {
            break;
        };
        tried.push((o.file.clone(), o.line, o.col));
        let new_name = &variants[o.variant].to;
        let (line, character) = originals
            .get(&o.file)
            .map(|text| lsp_position(text, &o))
            .unwrap_or((o.line.saturating_sub(1), o.col.saturating_sub(1)));
        let edit = match rename_symbol(remote, root, &o.file, line, character, new_name).await {
            Ok(edit) => edit,
            Err(err) => {
                notes.push(format!(
                    "{}:{}:{} — rename refused: {}",
                    display(root, &o.file),
                    o.line,
                    o.col,
                    format!("{err:#}").lines().next().unwrap_or("")
                ));
                continue;
            }
        };
        let parts = ranged_edits(&edit);
        if parts.is_empty() {
            notes.push(format!(
                "{}:{}:{} — the analyzer rewrote nothing",
                display(root, &o.file),
                o.line,
                o.col
            ));
            continue;
        }
        // A rename that wants characters another rename already took is not merged: the two
        // results were both computed against the file as it is now, and applying both would
        // produce text neither of them meant.
        let collides = parts.iter().any(|(path, edits, replaces_file)| {
            let taken = claimed.get(path).map(|v| v.as_slice()).unwrap_or_default();
            let mine_replaces = *replaces_file;
            (mine_replaces && !taken.is_empty())
                || whole.contains_key(path) && !edits.is_empty()
                || edits.iter().any(|e| {
                    let span = span_of(e);
                    taken.iter().any(|other| overlaps(span, *other))
                })
        });
        if collides {
            notes.push(format!(
                "{}:{}:{} — another rename already changes these characters; apply this run and \
                 run it again to finish",
                display(root, &o.file),
                o.line,
                o.col
            ));
            continue;
        }
        for (path, edits, replaces_file) in parts {
            if replaces_file {
                let text = edits
                    .first()
                    .and_then(|e| e.get("newText"))
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .to_string();
                // Which occurrences the new text covered can only be seen by looking at it:
                // the ones whose spelling is gone are done, and one still in place belongs to
                // another symbol, which needs a rename of its own on a later run.
                done.extend(
                    found
                        .iter()
                        .filter(|o| {
                            o.file == path && !still_spelled(&text, o, &variants[o.variant])
                        })
                        .map(|o| (o.file.clone(), o.line, o.col)),
                );
                whole.insert(path.clone(), text);
                claimed
                    .entry(path.clone())
                    .or_default()
                    .push((0, 0, u32::MAX, 0));
            } else {
                for e in &edits {
                    let span = span_of(e);
                    claimed.entry(path.clone()).or_default().push(span);
                    done.push((path.clone(), span.0 + 1, span.1 + 1));
                }
                ranged.entry(path).or_default().extend(edits);
            }
        }
    }

    // What the analyzers made of each file they touched, plus every file the scan found.
    let mut base: BTreeMap<PathBuf, String> = originals.clone();
    for (path, text) in whole {
        base.insert(path, text);
    }
    for (path, edits) in ranged {
        let before = match base.get(&path) {
            Some(text) => text.clone(),
            None => std::fs::read_to_string(&path).unwrap_or_default(),
        };
        let after = crate::refactor::apply_text_edits(&before, &edits)
            .with_context(|| format!("applying the rename to {}", display(root, &path)))?;
        base.insert(path, after);
    }

    // Phase two: the text the analyzers do not own, found again in what they produced.
    let mut rewritten: Vec<(PathBuf, String)> = Vec::new();
    let mut left: Vec<String> = Vec::new();
    let mut as_text: BTreeMap<&'static str, usize> = BTreeMap::new();
    let mut remaining: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (path, text) in &base {
        let kind = kind_of(path);
        let language = label(path, text);
        let schema = schema_of(path, text);
        let hits = scan(text, &variants, path);
        let mut edits = Vec::new();
        for o in &hits {
            let Some(language) = language else {
                continue;
            };
            let (ours, why) = text_rewrite_decision(kind, schema, text, o);
            if ours {
                *as_text.entry(language).or_default() += 1;
                edits.push(edit_for(o, &variants[o.variant]));
            } else {
                *remaining.entry(language).or_default() += 1;
                left.push(format!(
                    "{}:{}:{} `{}`{why}",
                    display(root, path),
                    o.line,
                    o.col,
                    variants[o.variant].from
                ));
            }
        }
        let after = if edits.is_empty() {
            text.clone()
        } else {
            // `scan` counts columns in characters, not in the UTF-16 units an analyzer's
            // edits use.
            crate::refactor::apply_scalar_text_edits(text, &edits)
                .with_context(|| format!("rewriting the text of {}", display(root, path)))?
        };
        let on_disk = originals
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        if after != on_disk {
            rewritten.push((path.clone(), after));
        }
    }
    rewritten.sort_by(|a, b| a.0.cmp(&b.0));
    let mut original_lines: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut original_ends: BTreeMap<PathBuf, (u32, u32)> = BTreeMap::new();
    for (path, _) in &rewritten {
        let raw = originals
            .get(path)
            .cloned()
            .or_else(|| std::fs::read_to_string(path).ok())
            .unwrap_or_default();
        original_lines.insert(path.clone(), raw.lines().count());
        original_ends.insert(path.clone(), lsp_end_position(&raw));
    }
    left.extend(notes);

    // Per language: what the scan found, and what became of it.
    let mut summary = Vec::new();
    let mut by_language: BTreeMap<&'static str, usize> = BTreeMap::new();
    for o in &found {
        if let Some(l) = originals.get(&o.file).and_then(|text| label(&o.file, text)) {
            *by_language.entry(l).or_default() += 1;
        }
    }
    for (language, total) in &by_language {
        let text = as_text.get(language).copied().unwrap_or(0);
        let still = remaining.get(language).copied().unwrap_or(0);
        let semantic = total.saturating_sub(text + still);
        summary.push(format!(
            "{language}: {total} occurrence(s) found, {semantic} renamed by the analyzer, \
             {text} rewritten as text, {still} left alone"
        ));
    }
    let unseen = rewritten
        .iter()
        .filter(|(p, _)| !originals.contains_key(p))
        .count();
    if unseen > 0 {
        summary.push(format!(
            "{unseen} file(s) the scan never looked at were updated by an analyzer, because the \
             symbol reaches them"
        ));
    }
    summary.push(format!(
        "spellings looked for: {}",
        variants
            .iter()
            .map(|v| format!("`{}` ({})", v.from, v.style))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // Every project checked by its own analyzer: one language's engine cannot judge another's.
    let mut by_project: BTreeMap<String, Vec<(PathBuf, String)>> = BTreeMap::new();
    for (path, text) in &rewritten {
        if !matches!(kind_of(path), Kind::Code(_)) {
            continue;
        }
        let (subdir, _) = crate::sync::engine_project(root, path);
        by_project
            .entry(subdir.unwrap_or_default())
            .or_default()
            .push((path.clone(), text.clone()));
    }
    let mut diagnostics = Vec::new();
    for group in by_project.values() {
        let reports = crate::diagnostics::validate_texts(remote, root, group, &[]).await?;
        for report in &reports {
            for d in report.items.iter().filter(|d| d.severity == "error") {
                diagnostics.push(format!(
                    "{}{} ({}:{}:{})",
                    d.message.lines().next().unwrap_or(""),
                    d.code
                        .as_deref()
                        .map(|c| format!(" [{c}]"))
                        .unwrap_or_default(),
                    report.file,
                    d.line,
                    d.col
                ));
            }
        }
    }

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the rename does not compile ({} error(s)); nothing was written. Pass `force: true` \
             to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        write_rewritten(root, &rewritten, &original_ends)?;
        applied = true;
    }

    Ok(SchemaRename {
        field: field.to_string(),
        to: to.to_string(),
        root: root.to_path_buf(),
        rewritten,
        original_lines,
        original_ends,
        summary,
        left,
        diagnostics,
        applied,
    })
}
