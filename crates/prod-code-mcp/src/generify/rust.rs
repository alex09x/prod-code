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
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use super::syntax::{display, generics_span, is_ident, split_reference};
use super::types::Generified;

/// Makes the parameter `param` of the function declared at `line`:`col` of `file` generic in Rust.
#[allow(clippy::too_many_arguments)]
pub async fn generify_rust(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    param: &str,
    bound: &str,
    type_param: &str,
    apply: bool,
    force: bool,
) -> Result<Generified> {
    anyhow::ensure!(
        !type_param.is_empty() && type_param.chars().all(is_ident),
        "`{type_param}` is not a type parameter name"
    );
    let bound = bound.trim();
    anyhow::ensure!(
        !bound.is_empty(),
        "give the `bound` the parameter's type must satisfy"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    let at = crate::signature::offset_of(&text, line, col)
        .context("the position is not inside the file")?;
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_ident(*c))
        .last()
        .map_or(at, |(i, _)| i);
    anyhow::ensure!(
        text[..start].trim_end().ends_with("fn"),
        "the position is not the name of a function declaration"
    );
    let (name, open, close) =
        crate::signature::param_span(&text, start).context("the function has no parameter list")?;
    let name_end = start + name.len();

    // The parameter, and the span of its type.
    let list = &text[open..close];
    let mut offset = open;
    let mut found = None;
    for part in crate::signature::split_params(list) {
        let at_in = list[offset - open..]
            .find(part.trim())
            .map(|i| offset + i)
            .unwrap_or(offset);
        let (pattern, ty) = part.split_once(':').unwrap_or((part.as_str(), ""));
        let pattern = pattern.trim().trim_start_matches("mut ").trim();
        if pattern == param {
            let ty_start = at_in + part.trim().find(':').map_or(0, |i| i + 1);
            let lead = text[ty_start..].len() - text[ty_start..].trim_start().len();
            let ty_trim = ty.trim();
            found = Some((
                ty_start + lead,
                ty_start + lead + ty_trim.len(),
                ty_trim.to_string(),
            ));
            break;
        }
        offset = at_in + part.trim().len();
    }
    let (ty_start, ty_end, ty) =
        found.with_context(|| format!("`{name}` has no parameter `{param}`"))?;
    let (reference, concrete) = split_reference(&ty);
    anyhow::ensure!(
        !concrete.starts_with("impl ") && !concrete.starts_with("dyn "),
        "`{param}: {ty}` is already abstract"
    );
    let generics = generics_span(&text, name_end);
    if let Some((g_start, g_end)) = generics {
        let existing = &text[g_start..g_end];
        anyhow::ensure!(
            !existing
                .split(|c: char| !is_ident(c))
                .any(|word| word == type_param),
            "`{name}` already has a generic parameter `{type_param}`; pass another `type_param`"
        );
    }

    let was = text[text[..start].rfind("fn").unwrap_or(start)..close + 1].to_string();
    let mut new_text = text.clone();
    new_text.replace_range(ty_start..ty_end, &format!("{reference}{type_param}"));
    match generics {
        Some((_, g_end)) => {
            let existing = text[..g_end].trim_end();
            let sep = if existing.ends_with('<') || existing.ends_with(',') {
                ""
            } else {
                ", "
            };
            new_text.insert_str(g_end, &format!("{sep}{type_param}: {bound}"));
        }
        None => new_text.insert_str(name_end, &format!("<{type_param}: {bound}>")),
    }
    let fn_at = new_text[..start].rfind("fn").unwrap_or(start);
    let now_end = crate::signature::param_span(&new_text, start)
        .map(|(_, _, c)| c + 1)
        .unwrap_or(new_text.len());
    let now = new_text[fn_at..now_end].to_string();

    // Every file that calls it is checked against the new signature.
    let (nl, nc) = crate::signature::position_at(&text, start)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    // Without them the callers go unchecked, and a clean report would mean nothing (#446).
    let callers: BTreeSet<PathBuf> = crate::signature::references(remote, root, file, nl, nc)
        .await
        .context("cannot find the callers to check against the new signature; nothing was planned")?
        .into_iter()
        .map(|(p, _, _)| p)
        .filter(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) != canonical)
        .collect();
    let also: Vec<PathBuf> = callers.iter().cloned().collect();
    let reports = crate::diagnostics::validate_texts(
        remote,
        root,
        &[(file.to_path_buf(), new_text.clone())],
        &also,
    )
    .await?;
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
            "the change does not compile ({} error(s)); nothing was written. The body \
             needs more than the bound promises, or a caller no longer satisfies it or can no longer \
             infer its type (an `.into()` that took its target from the old type); choose another \
             bound, fix the caller, or pass `force: true`:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let files: std::collections::BTreeMap<PathBuf, String> =
            std::iter::once((file.to_path_buf(), new_text.clone())).collect();
        crate::refactor::apply_workspace_edit(root, &crate::signature::whole_file_edit(&files))?;
        applied = true;
    }

    Ok(Generified {
        function: name,
        root: root.to_path_buf(),
        file: display(root, file),
        was,
        now,
        callers_checked: callers.len(),
        rewritten: vec![(file.to_string_lossy().into_owned(), new_text)],
        diagnostics,
        applied,
    })
}
