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
use crate::signature_go::add_plan::{
    addition_groups, addition_plan, insertion_edits, requested_arguments, requested_go_params,
};
use crate::signature_go::evidence::{function_reference_evidence, receiver_interface_evidence};
use crate::signature_go::interface::ordinary_receiver;
use crate::signature_go::parse::{body_open, header, identifier_uses, parameters};
use crate::signature_go::syntax::{canonical, list_text, normalize, suffix};
use crate::signature_go::text::{
    closing, comments, display, map_offset, position, splice, split_list, strip_comments,
};
use crate::signature_go::types::{Decl, GoParam, TextEdit, refusal};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Adds explicitly typed primitive parameters to an ordinary function or named value/pointer
/// receiver method. Unlike reorders and removals, gopls has no native edit for this shape, so its
/// complete reference answer is used as the proof obligation and the adapter makes insertion-only
/// edits itself.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn add_parameters(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: String,
    decl: Decl,
    declared: Vec<GoParam>,
    request: &[Param],
    apply: bool,
) -> Result<SignatureChange> {
    let receiver = decl
        .receiver
        .as_deref()
        .map(ordinary_receiver)
        .transpose()
        .map_err(|why| {
            refusal(format!(
                "adding parameters to `{}` is refused: {why}",
                decl.name
            ))
        })?;
    anyhow::ensure!(
        !decl.generic,
        "{}",
        refusal(format!(
            "adding parameters to the generic function `{}` is not supported",
            decl.name
        ))
    );
    anyhow::ensure!(
        !declared.iter().any(|p| p.ty.starts_with("...")),
        "{}",
        refusal(format!(
            "adding parameters to the variadic function `{}` is not supported",
            decl.name
        ))
    );
    let body_open = body_open(&text, decl.close + 1).with_context(|| {
        refusal(format!(
            "adding parameters to `{}` is refused because it has no body here (an assembly or \
             external declaration cannot be proven safe)",
            decl.name
        ))
    })?;
    let body = (
        body_open,
        closing(&text, body_open)
            .with_context(|| refusal(format!("the body of `{}` does not close", decl.name)))?,
    );
    let additions = addition_plan(&declared, request)?;
    for added in &additions {
        if receiver
            .as_ref()
            .is_some_and(|receiver| receiver.binding == added.name)
        {
            return Err(refusal(format!(
                "adding `{}` is refused because it duplicates the receiver binding of `{}`",
                added.name, decl.name
            )));
        }
        let captures: Vec<String> = identifier_uses(&text, body.0, body.1, &added.name)
            .into_iter()
            .map(|at| position(root, file, &text, at))
            .collect();
        if !captures.is_empty() {
            return Err(refusal(format!(
                "adding `{}` is refused because it would shadow existing references in the body \
                 of `{}` at {}",
                added.name,
                decl.name,
                captures.join(", ")
            )));
        }
    }

    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve the checkout {}", root.display()))?;
    if receiver.is_some() {
        receiver_interface_evidence(remote, root, file, &text, &decl.name, decl.name_at)
            .await
            .map_err(|why| {
                refusal(format!(
                    "adding parameters to `{}` is refused because its interface implementations \
                     cannot be proven absent: {why:#}",
                    decl.name
                ))
            })?;
    }
    let (originals, calls) = function_reference_evidence(
        remote,
        root,
        file,
        &canonical_root,
        &text,
        &decl,
        receiver.as_ref().map(|receiver| receiver.ty.as_str()),
    )
    .await
    .map_err(|why| {
        refusal(format!(
            "cannot prove every reference to `{}` is a supported direct call or its declaration: \
             {why:#}",
            decl.name
        ))
    })?;
    let arity = declared.len();
    for call in &calls {
        anyhow::ensure!(
            call.args.len() == arity
                && !call
                    .args
                    .last()
                    .is_some_and(|arg| arg.trim_end().ends_with("...")),
            "{}",
            refusal(format!(
                "{}: the call passes {} argument(s) and `{}` declares {arity}, so the insertion \
                 cannot be reconciled exactly",
                call.at,
                call.args.len(),
                decl.name
            ))
        );
    }

    let groups = addition_groups(&additions);
    let mut edits: BTreeMap<PathBuf, Vec<TextEdit>> = BTreeMap::new();
    let declaration_edits = insertion_edits(&text, decl.open, decl.close, arity, &groups, true)
        .map_err(|why| {
            refusal(format!(
                "the declaration of `{}` cannot be extended safely: {why}",
                decl.name
            ))
        })?;
    edits
        .entry(file.to_path_buf())
        .or_default()
        .extend(declaration_edits);
    for call in &calls {
        let old = &originals[&call.path];
        let call_edits = insertion_edits(old, call.open, call.close, arity, &groups, false)
            .map_err(|why| refusal(format!("{} cannot be extended safely: {why}", call.at)))?;
        edits
            .entry(call.path.clone())
            .or_default()
            .extend(call_edits);
    }
    for list in edits.values_mut() {
        list.sort_by_key(|(start, end, _)| (*start, *end));
        for pair in list.windows(2) {
            anyhow::ensure!(
                pair[0].1 <= pair[1].0 && !(pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1),
                "the insertion plan for `{}` overlaps itself; nothing was written",
                decl.name
            );
        }
    }

    let mut rewritten = BTreeMap::new();
    for (path, list) in &edits {
        let old = &originals[path];
        let new = splice(old, list);
        anyhow::ensure!(
            comments(old) == comments(&new),
            "{}: adding parameters would drop or change a comment; nothing was written",
            display(root, path)
        );
        rewritten.insert(path.clone(), new);
    }

    let expected_params = requested_go_params(&declared, &additions);
    let new_decl = edits
        .get(file)
        .and_then(|list| map_offset(list, decl.func_at))
        .and_then(|at| header(&rewritten[file], at))
        .context("the inserted declaration cannot be read back; nothing was written")?;
    let got_params =
        parameters(&rewritten[file][new_decl.open + 1..new_decl.close]).map_err(|why| {
            anyhow::anyhow!(
                "the inserted declaration cannot be read back: {why}; nothing was written"
            )
        })?;
    anyhow::ensure!(
        new_decl.name == decl.name
            && new_decl.receiver == decl.receiver
            && new_decl.results == decl.results
            && got_params == expected_params,
        "the insertion did not produce exactly the requested declaration of `{}`; nothing was written",
        decl.name
    );
    for call in &calls {
        let list = &edits[&call.path];
        let open = map_offset(list, call.open)
            .context("an inserted call cannot be located again; nothing was written")?;
        let new = &rewritten[&call.path];
        let close =
            closing(new, open).context("an inserted call does not close; nothing was written")?;
        let got = split_list(&strip_comments(&new[open + 1..close]));
        let expected = requested_arguments(&call.args, request);
        let has_nested_call = calls.iter().any(|nested| {
            nested.path == call.path && call.open < nested.open && nested.close < call.close
        });
        let arguments_match =
            got.iter()
                .zip(&expected)
                .zip(request)
                .all(|((actual, expected), item)| {
                    if has_nested_call && matches!(item, Param::Keep(_)) {
                        // A nested call's own insertion legitimately changes this original argument;
                        // that nested call is validated against its own request in this same loop.
                        true
                    } else {
                        canonical(actual) == canonical(expected)
                    }
                });
        anyhow::ensure!(
            got.len() == expected.len() && arguments_match,
            "{}: the insertion wrote ({}) instead of exactly ({}); nothing was written",
            call.at,
            got.join(", "),
            expected.join(", ")
        );
    }

    let proposal: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(path, source)| (path.clone(), source.clone()))
        .collect();
    let compiler = crate::verify::compile_go_shadow(remote, root, file, &proposal)
        .await
        .context(
            "the complete Go proposal could not be compiled in its private shadow; nothing was written",
        )?;
    anyhow::ensure!(
        compiler.passed,
        "the changed Go project does not compile; nothing was written, and `force` does not \
         override this:\n{}",
        compiler.output.trim()
    );

    let mut applied = false;
    if apply {
        for (path, old) in &originals {
            let now = std::fs::read_to_string(path).unwrap_or_default();
            anyhow::ensure!(
                now == *old,
                "{} changed while the addition was planned and compiled; nothing was written",
                display(root, path)
            );
        }
        crate::refactor::apply_workspace_edit(
            root,
            &crate::signature::whole_file_edit(&rewritten),
        )?;
        applied = true;
    }

    let signature = format!(
        "func({}){}",
        list_text(&expected_params),
        suffix(&decl.results)
    );
    let new_signature = normalize(&rewritten[file][new_decl.open + 1..new_decl.close]);
    Ok(SignatureChange {
        symbol: decl.name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&text[decl.open + 1..decl.close]),
        new_signature,
        rule: signature,
        rewritten: rewritten
            .into_iter()
            .map(|(path, source)| (path.to_string_lossy().into_owned(), source))
            .collect(),
        unmatched: Vec::new(),
        unexpected: Vec::new(),
        diagnostics: Vec::new(),
        applied,
        returns: None,
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}
