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

use super::parse::{impl_blocks, parse_struct};
use super::restructure::restructure;
use super::types::Extracted;
use crate::extract_delegate::common::is_ident;

/// Extracts `fields` and `methods` of a struct into a helper struct `helper`, held
/// in the new field `field` for Rust.
#[allow(clippy::too_many_arguments)]
pub async fn extract_delegate_rust(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    symbol: Option<&str>,
    line: Option<u32>,
    col: Option<u32>,
    fields: &[String],
    methods: &[String],
    helper: &str,
    field: &str,
    apply: bool,
    force: bool,
) -> Result<Extracted> {
    for n in [helper, field] {
        anyhow::ensure!(
            !n.is_empty() && n.chars().all(is_ident),
            "`{n}` is not an identifier"
        );
    }
    anyhow::ensure!(!fields.is_empty(), "name at least one field");
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = if let (Some(l), Some(c)) = (line, col)
        && l > 0
        && c > 0
    {
        crate::signature::offset_of(&text, l, c).context("the position is not in the file")?
    } else if let Some(sym) = symbol {
        let needle = format!("struct {sym}");
        text.find(&needle)
            .with_context(|| format!("struct `{sym}` not found in {}", file.display()))?
    } else {
        anyhow::bail!("provide either line and character or symbol");
    };
    let decl = parse_struct(&text, at)?;
    // Check the struct side first, so a wrong name fails before any analyzer query.
    restructure(&text, at, fields, methods, helper, field)?;

    // Self receivers and their local aliases in moved methods belong to the helper. Other owner
    // instances in the same method still need their delegate field.
    let moved_ranges: Vec<(usize, usize)> = impl_blocks(&text, &decl.name)
        .into_iter()
        .flat_map(|(_, open, close)| crate::extract_trait::items(&text, open, close))
        .filter(|i| i.name.as_ref().is_some_and(|n| methods.contains(n)))
        .map(|i| (i.start, i.end))
        .collect();
    let mut occurrences: BTreeMap<PathBuf, Vec<(usize, String)>> = BTreeMap::new();
    let mut files = BTreeMap::from([(file.to_path_buf(), text.clone())]);
    let body = &text[decl.open + 1..decl.close];
    for f in fields {
        let Some(rel_at) = body.match_indices(f.as_str()).map(|(i, _)| i).find(|i| {
            !body[..*i].chars().next_back().is_some_and(is_ident)
                && body[i + f.len()..].trim_start().starts_with(':')
        }) else {
            continue;
        };
        let (fl, fc) = crate::signature::position_at(&text, decl.open + 1 + rel_at)?;
        let refs = crate::signature::references(remote, root, file, fl, fc)
            .await
            .with_context(|| format!("cannot find the uses of `{f}`; nothing was planned"))?;
        for (path, rl, rc) in refs {
            let other = if path == file {
                text.clone()
            } else {
                std::fs::read_to_string(&path).with_context(|| {
                    format!(
                        "cannot read {}, where the analyzer reports a use of `{f}`; nothing was \
                         planned",
                        path.display()
                    )
                })?
            };
            // An access passed over would still name a field the struct no longer has (#446).
            let off = crate::signature::offset_of(&other, rl, rc).with_context(|| {
                format!(
                    "the analyzer places a use of `{f}` at {}:{rl}:{rc}, which is not in the \
                     file; nothing was planned",
                    path.display()
                )
            })?;
            // A stale position names something else, and a prefix there would break it.
            anyhow::ensure!(
                other[off..].starts_with(f.as_str())
                    && !other[off + f.len()..].starts_with(is_ident)
                    && !other[..off].chars().next_back().is_some_and(is_ident),
                "the analyzer places a use of `{f}` at {}:{rl}:{rc}, but the file says otherwise; \
                 nothing was planned",
                path.display()
            );
            files.entry(path.clone()).or_insert(other);
            occurrences.entry(path).or_default().push((off, f.clone()));
        }
    }
    let mut accesses = 0;
    for (path, mut refs) in occurrences {
        refs.sort_unstable();
        refs.dedup();
        let ranges = if path == file {
            moved_ranges.as_slice()
        } else {
            &[]
        };
        let (rewritten, count) =
            super::occurrences::rewrite(&files[&path], &refs, ranges, fields, field, |off| {
                super::helper_path::spelling(
                    remote,
                    root,
                    file,
                    &text,
                    &decl,
                    &path,
                    &files[&path],
                    off,
                    helper,
                )
            })
            .await?;
        accesses += count;
        files.insert(path, rewritten);
    }
    // The declaring file: the struct, the helper and the methods.
    let declaring = files.get(file).cloned().unwrap_or_default();
    let at_now = declaring
        .find(&format!("struct {} ", decl.name))
        .unwrap_or(at);
    let restructured = restructure(&declaring, at_now, fields, methods, helper, field)?;
    files.insert(file.to_path_buf(), restructured);
    files.retain(|p, t| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true));

    let edits: Vec<(PathBuf, String)> = files.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
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
            "the change does not compile ({} error(s)); nothing was written:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }
    Ok(Extracted {
        helper: helper.to_string(),
        field: field.to_string(),
        fields: fields.to_vec(),
        methods: methods.to_vec(),
        root: root.to_path_buf(),
        rewritten: files
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        accesses,
        unmatched: Vec::new(),
        diagnostics,
        applied,
    })
}
