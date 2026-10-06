/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::{Param, SignatureChange};
use crate::signature_go::edits::{edits_by_file, ensure_inside, same_file};
use crate::signature_go::evidence::parameter_uses;
use crate::signature_go::hazards::{effect_hazards, is_subsequence, permutation, removed_names};
use crate::signature_go::parse::{
    body_open, call_parens, ident_at, identifier_uses, parameter_names_at,
};
use crate::signature_go::reorder_verify::verify_gopls_edits;
use crate::signature_go::syntax::normalize;
use crate::signature_go::text::{
    closing, display, line_col_utf16, offset_at, position, split_list, strip_comments,
};
use crate::signature_go::types::{Call, Decl, GoParam, STILL_OPEN, refusal};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub(crate) async fn reorder_or_remove_parameters(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: String,
    decl: Decl,
    declared: Vec<GoParam>,
    request: &[Param],
    apply: bool,
) -> Result<SignatureChange> {
    let order = permutation(&declared, request)?;
    let arity = declared.len();
    let variadic = declared.last().is_some_and(|p| p.ty.starts_with("..."));
    if variadic && order.contains(&(arity - 1)) && order.last() != Some(&(arity - 1)) {
        return Err(refusal(format!(
            "`{}` is variadic, and Go allows `...` only on the last parameter",
            declared[arity - 1].name
        )));
    }
    let removed: Vec<usize> = (0..arity).filter(|i| !order.contains(i)).collect();
    let kind = match (removed.is_empty(), is_subsequence(&order)) {
        (true, _) => "reorder",
        (false, true) => "removal",
        (false, false) => "removal and reorder",
    };
    // The body, and where each removed parameter is named in the declaration: what the proof
    // that it is unused is about.
    let mut removed_at = Vec::new();
    let mut body = (0, 0);
    if !removed.is_empty() {
        let names = removed_names(&declared, &removed);
        if decl.generic {
            return Err(refusal(format!(
                "removing {names} from the generic `{}` is not supported: a type argument may be \
                 inferred from the removed argument, and gopls cannot change a generic \
                 function's signature while it has calls",
                decl.name
            )));
        }
        body = body_open(&text, decl.close + 1)
            .and_then(|open| closing(&text, open).map(|close| (open, close)))
            .with_context(|| {
                refusal(format!(
                    "removing {names} is refused: `{}` has no body here (a function implemented \
                     in assembly reads its arguments by position)",
                    decl.name
                ))
            })?;
        let named_at = parameter_names_at(&text, decl.open, decl.close);
        for &i in &removed {
            let name = &declared[i].name;
            let at = named_at
                .get(i)
                .copied()
                .filter(|&at| ident_at(&text, at) == Some(name.as_str()))
                .with_context(|| {
                    refusal(format!(
                        "removing `{name}` is refused: its name cannot be found in the \
                         declaration of `{}`",
                        decl.name
                    ))
                })?;
            let uses: Vec<String> = identifier_uses(&text, body.0, body.1, name)
                .into_iter()
                .map(|o| position(root, file, &text, o))
                .collect();
            if !uses.is_empty() {
                return Err(refusal(format!(
                    "removing `{name}` is refused: the body of `{}` still uses it at {}",
                    decl.name,
                    uses.join(", ")
                )));
            }
            removed_at.push((i, at));
        }
    }
    let new_params: Vec<GoParam> = order.iter().map(|&i| declared[i].clone()).collect();
    let signature = format!(
        "func({}){}",
        new_params
            .iter()
            .map(|p| format!("{} {}", p.name, p.ty))
            .collect::<Vec<_>>()
            .join(", "),
        if decl.results.is_empty() {
            String::new()
        } else {
            format!(" {}", decl.results)
        }
    );

    // Every reference, before anything is asked of gopls: what cannot be rewritten or would run
    // differently is refused with its location.
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve the checkout {}", root.display()))?;
    let (name_line, name_col) = line_col_utf16(&text, decl.name_at);
    let mut refs = crate::signature::references(remote, root, file, name_line + 1, name_col + 1)
        .await
        .with_context(|| {
            format!(
                "cannot list the references to `{}`; nothing was written",
                decl.name
            )
        })?;
    refs.sort();
    refs.dedup();
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    originals.insert(file.to_path_buf(), text.clone());
    let mut calls = Vec::new();
    let mut values = Vec::new();
    for (reported, l, c) in &refs {
        ensure_inside(&canonical_root, reported)?;
        // One spelling per file, the declaring file's own when the reference is in it.
        let path = originals
            .keys()
            .find(|k| same_file(k, reported))
            .cloned()
            .unwrap_or_else(|| reported.clone());
        if !originals.contains_key(&path) {
            let t = std::fs::read_to_string(&path)
                .with_context(|| format!("cannot read {}; nothing was written", path.display()))?;
            originals.insert(path.clone(), t);
        }
        let t = &originals[&path];
        let where_ = format!("{}:{l}:{c}", display(root, &path));
        let name_at = offset_at(t, l.saturating_sub(1), c.saturating_sub(1))
            .filter(|o| t[*o..].starts_with(decl.name.as_str()))
            .with_context(|| {
                format!(
                    "the reference {where_} does not point at `{}`; the file may have changed \
                     since it was read. Nothing was written",
                    decl.name
                )
            })?;
        match call_parens(t, name_at) {
            Some((open, close)) => calls.push(Call {
                path: path.clone(),
                at: where_,
                open,
                close,
                args: split_list(&strip_comments(&t[open + 1..close])),
            }),
            None => values.push(where_),
        }
    }
    if !values.is_empty() {
        return Err(refusal(format!(
            "`{}` is used as a value, not called, at {}; a function value keeps the old \
             signature and cannot be rewritten",
            decl.name,
            values.join(", ")
        )));
    }
    // The analyzer's word that each removed parameter is unused: its declaration, and nothing else.
    for &(i, at) in &removed_at {
        let name = &declared[i].name;
        let uses = parameter_uses(remote, root, file, &text, name, at, body)
            .await
            .map_err(|why| {
                refusal(format!(
                    "removing `{name}` is refused: gopls's references cannot prove it unused: \
                     {why:#}"
                ))
            })?;
        if !uses.is_empty() {
            return Err(refusal(format!(
                "removing `{name}` is refused: the body of `{}` still uses it at {} (gopls)",
                decl.name,
                uses.iter()
                    .map(|&o| position(root, file, &text, o))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    let hazards = effect_hazards(&decl.name, &declared, &order, variadic, &calls);
    anyhow::ensure!(
        hazards.is_empty(),
        "the new signature would change what the program does, not only how the calls are \
         written; nothing was written, and `force` does not override this:\n  {}\nbind such an \
         argument to a local before the call and pass the local",
        hazards.join("\n  ")
    );

    // gopls writes the change.
    let (func_line, func_col) = line_col_utf16(&text, decl.func_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let edit = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/rename",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": func_line, "character": func_col },
            "newName": signature,
        }),
    )
    .await
    .map_err(|e| {
        let generic = if decl.generic {
            " (gopls cannot reorder a generic function's parameters while it has calls)"
        } else {
            ""
        };
        anyhow::anyhow!(
            "gopls refused the {kind} of `{}` as `{signature}`{generic}: {e:#}; nothing was \
             written. {STILL_OPEN}",
            decl.name
        )
    })?;
    anyhow::ensure!(
        !edit.is_null(),
        "gopls answered the {kind} of `{}` with no edit; nothing was written. {STILL_OPEN}",
        decl.name
    );
    let edits = edits_by_file(&canonical_root, &edit, &mut originals)?;

    // What gopls wrote, against what was asked.
    let mut rewritten: BTreeMap<PathBuf, String> = BTreeMap::new();
    let verification = verify_gopls_edits(
        root,
        file,
        &text,
        &decl,
        &new_params,
        &order,
        arity,
        variadic,
        &calls,
        &edits,
        &originals,
        &mut rewritten,
    );
    let unexpected = verification.unexpected;
    let unmatched = verification.unmatched;
    let new_signature = verification.new_signature;
    // The whole proposal, judged together; files that call the name but were not reported are
    // checked with it.
    let proposal: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let mut also: Vec<PathBuf> = originals
        .keys()
        .filter(|p| !rewritten.contains_key(*p))
        .cloned()
        .collect();
    let checked: Vec<PathBuf> = originals.keys().cloned().collect();
    also.extend(crate::signature::unreported_callers(
        root, file, &decl.name, &checked,
    ));
    let reports = crate::diagnostics::validate_texts(remote, root, &proposal, &also)
        .await
        .context("the proposal could not be validated; nothing was written")?;
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
            unmatched.is_empty() && unexpected.is_empty(),
            "gopls's edit is not the {kind} that was asked for; nothing was written:\n  {}",
            unmatched
                .iter()
                .chain(&unexpected)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n  ")
        );
        anyhow::ensure!(
            diagnostics.is_empty(),
            "the changed program does not type-check ({} error(s)); nothing was written, and \
             `force` does not override this:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        for (path, old) in &originals {
            let now = std::fs::read_to_string(path).unwrap_or_default();
            anyhow::ensure!(
                now == *old,
                "{} changed while the {kind} was planned; nothing was written",
                display(root, path)
            );
        }
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }

    Ok(SignatureChange {
        symbol: decl.name.clone(),
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&text[decl.open + 1..decl.close]),
        new_signature,
        rule: signature.clone(),
        rewritten: rewritten
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        unmatched,
        unexpected,
        diagnostics,
        applied,
        returns: None,
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}
