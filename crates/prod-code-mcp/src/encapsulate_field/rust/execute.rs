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

use super::super::case::{display, is_ident};
use super::super::types::{Access, EncapsulatedField};
use super::accessors::accessors;
use super::analysis::{access_at, field_at, inherent_impl, is_generic, owner_at, returns_by_value};

/// Makes the field declared at `line`:`col` of `file` private and rewrites every access to it
/// outside that file into a call of its getter or setter.
#[allow(clippy::too_many_arguments)]
pub async fn encapsulate(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    by_value: Option<bool>,
    apply: bool,
    force: bool,
) -> Result<EncapsulatedField> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let offset = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let decl = field_at(&text, offset)?;
    let field = decl.name.clone();
    anyhow::ensure!(
        !decl.vis.trim().is_empty(),
        "`{field}` is already private; there is nothing outside its module to rewrite"
    );
    let (owner, struct_at, struct_close) = owner_at(&text, decl.name_at)
        .with_context(|| format!("`{field}` is not a field of a struct with named fields"))?;
    let by_value = by_value.unwrap_or_else(|| returns_by_value(&decl.ty));

    // Every reference outside the declaring file, classified by what it does there.
    let (name_line, name_col) = crate::signature::position_at(&text, decl.name_at)?;
    let mut edits: BTreeMap<PathBuf, Vec<(usize, usize, String)>> = BTreeMap::new();
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let (mut reads, mut writes, mut chained_reads, mut left_in_file) = (0, 0, 0, 0);
    let mut blocked = Vec::new();
    let mut unmatched = Vec::new();
    let refs = crate::signature::references(remote, root, file, name_line, name_col)
        .await
        .with_context(|| format!("cannot find the uses of `{field}`; nothing was planned"))?;
    for (path, rl, rc) in refs {
        if path == *file {
            left_in_file += 1;
            continue;
        }
        let body = crate::refactor::referenced_text(&mut texts, &path)?;
        let at_site = format!("{}:{rl}:{rc}", display(root, &path));
        let Some(at) = crate::signature::offset_of(body, rl, rc) else {
            unmatched.push(format!("{at_site} (the position is not in the file)"));
            continue;
        };
        // The analyzer's position is trusted only when the name is actually there (#75).
        if !body[at..].starts_with(field.as_str()) || body[at + field.len()..].starts_with(is_ident)
        {
            unmatched.push(format!(
                "{at_site} (the analyzer places `{field}` here, but the file says otherwise)"
            ));
            continue;
        }
        match access_at(body, at, field.len()) {
            Access::Read { chained } => {
                edits
                    .entry(path.clone())
                    .or_default()
                    .push((at + field.len(), 0, "()".into()));
                reads += 1;
                if chained {
                    chained_reads += 1;
                }
            }
            Access::Write { rhs } => {
                let value = body[rhs.0..rhs.1].trim().to_string();
                edits.entry(path.clone()).or_default().push((
                    at,
                    rhs.1 - at,
                    format!("set_{field}({value})"),
                ));
                writes += 1;
            }
            Access::Blocked(why) => {
                let source = body[..at].rfind('\n').map_or(0, |i| i + 1).min(body.len());
                let line_text = body[source..].lines().next().unwrap_or("").trim();
                blocked.push(format!("{at_site} {why}: `{line_text}`"));
            }
            Access::NotAccess => unmatched.push(format!("{at_site} (a call, not a field access)")),
        }
    }

    // The declaring file: the field loses its visibility and the accessors are added.
    let setter = writes > 0;
    for method in std::iter::once(field.clone()).chain(setter.then(|| format!("set_{field}"))) {
        anyhow::ensure!(
            !text.contains(&format!("fn {method}(")) && !text.contains(&format!("fn {method}<")),
            "`{owner}` already has a `fn {method}` in {}; rename it or the field first",
            display(root, file)
        );
    }
    let own = edits.entry(file.to_path_buf()).or_default();
    own.push((decl.vis_at, decl.name_at - decl.vis_at, String::new()));
    match inherent_impl(&text, &owner) {
        Some(open) => {
            let close = crate::parameter_object::matching_bracket(&text, open)
                .with_context(|| format!("the `impl {owner}` block does not close"))?;
            let line_start = text[..open].rfind('\n').map_or(0, |i| i + 1);
            let impl_indent: String = text[line_start..]
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            let methods = accessors(
                &format!("{impl_indent}    "),
                &decl.vis,
                &field,
                &decl.ty,
                by_value,
                setter,
            );
            let content_end = text[..close].trim_end().len();
            let gap = if content_end == open + 1 {
                "\n"
            } else {
                "\n\n"
            };
            own.push((
                content_end,
                close - content_end,
                format!("{gap}{}\n{impl_indent}", methods.trim_end()),
            ));
        }
        None => {
            anyhow::ensure!(
                !is_generic(&text, struct_at, &owner),
                "`{owner}` is generic and has no inherent `impl` in {} to put the accessors in; \
                 add an empty one first",
                display(root, file)
            );
            let methods = accessors("    ", &decl.vis, &field, &decl.ty, by_value, setter);
            own.push((
                struct_close + 1,
                0,
                format!("\n\nimpl {owner} {{\n{}\n}}", methods.trim_end()),
            ));
        }
    }
    texts.insert(file.to_path_buf(), text.clone());

    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    for (path, mut file_edits) in edits {
        let mut body = texts.get(&path).cloned().unwrap_or_default();
        file_edits.sort_by_key(|(at, _, _)| *at);
        for (at, len, replacement) in file_edits.into_iter().rev() {
            body.replace_range(at..at + len, &replacement);
        }
        rewritten.insert(path, body);
    }

    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, &[]).await?;
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
            blocked.is_empty() || force,
            "{} use(s) of `{field}` outside {} cannot become a method call, so a private field \
             would not compile there; nothing was written:\n  {}",
            blocked.len(),
            display(root, file),
            blocked.join("\n  ")
        );
        // A use left as it was reaches a private field in a file nothing here checks; `force`
        // overrides the analyzer, not a use this did not rewrite (#446).
        anyhow::ensure!(
            unmatched.is_empty(),
            "{} reference(s) to `{field}` were not rewritten; nothing was written:\n  {}",
            unmatched.len(),
            unmatched.join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Pass `force: true` \
             to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(&rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }

    Ok(EncapsulatedField {
        owner,
        root: root.to_path_buf(),
        file: display(root, file),
        field,
        ty: decl.ty,
        by_value,
        reads,
        writes,
        chained_reads,
        left_in_file,
        blocked,
        unmatched,
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied,
    })
}
