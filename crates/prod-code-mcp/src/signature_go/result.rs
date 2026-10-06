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
use crate::signature_go::evidence::{function_reference_evidence, receiver_interface_evidence};
use crate::signature_go::interface::ordinary_receiver;
use crate::signature_go::parse::body_open;
use crate::signature_go::shadowing::ensure_predeclared_types_unshadowed;
use crate::signature_go::syntax::{normalize, primitive_type};
use crate::signature_go::text::{closing, comments, display, skip_space, splice};
use crate::signature_go::types::{Decl, GoParam, refusal};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Replaces the sole unnamed primitive result of an ordinary free function or named value/pointer
/// receiver method. gopls cannot make this edit, and a result change can make an otherwise
/// untouched caller ill typed, so the
/// reference proof and compiler-shadow gate are mandatory even for a preview and even for a
/// no-op request.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn replace_result(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: String,
    decl: Decl,
    declared: Vec<GoParam>,
    request: &[Param],
    requested_result: &str,
    apply: bool,
) -> Result<SignatureChange> {
    let receiver = decl
        .receiver
        .as_deref()
        .map(ordinary_receiver)
        .transpose()
        .map_err(|why| {
            refusal(format!(
                "changing the result of `{}` is refused: {why}",
                decl.name
            ))
        })?;
    anyhow::ensure!(
        !decl.generic,
        "{}",
        refusal(format!(
            "changing the result of generic function `{}` is not supported",
            decl.name
        ))
    );
    anyhow::ensure!(
        !declared
            .iter()
            .any(|parameter| parameter.ty.starts_with("...")),
        "{}",
        refusal(format!(
            "changing the result of variadic function `{}` is not supported",
            decl.name
        ))
    );
    unchanged_parameters(&declared, request)?;
    anyhow::ensure!(
        primitive_type(&decl.results),
        "{}",
        refusal(format!(
            "changing the results of `{}` is supported only for one unnamed primitive result, not `{}`",
            decl.name,
            if decl.results.is_empty() {
                "no result"
            } else {
                &decl.results
            }
        ))
    );
    let requested_result = requested_result.trim();
    anyhow::ensure!(
        primitive_type(requested_result),
        "{}",
        refusal(format!(
            "the result of `{}` must be an ordinary primitive spelling, not `{requested_result}`",
            decl.name
        ))
    );
    ensure_predeclared_types_unshadowed(root, file, &[&decl.results, requested_result])
        .map_err(|why| {
            refusal(format!(
                "changing the result of `{}` is refused because primitive type identity cannot be proven: {why:#}",
                decl.name
            ))
        })?;
    let body = body_open(&text, decl.close + 1).with_context(|| {
        refusal(format!(
            "changing the result of `{}` is refused because it has no body here",
            decl.name
        ))
    })?;
    closing(&text, body)
        .with_context(|| refusal(format!("the body of `{}` does not close", decl.name)))?;

    // Read every path gopls names before considering a write. In particular, an indirect value,
    // stale coordinate or malformed reference is evidence we do not have, not an empty caller.
    let canonical_root = std::fs::canonicalize(root)
        .with_context(|| format!("cannot resolve the checkout {}", root.display()))?;
    if receiver.is_some() {
        receiver_interface_evidence(remote, root, file, &text, &decl.name, decl.name_at)
            .await
            .map_err(|why| {
                refusal(format!(
                    "changing the result of receiver method `{}` is refused: {why:#}",
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
                "{}: the call passes {} argument(s) and `{}` declares {arity}, so its result \
                 replacement cannot be reconciled exactly",
                call.at,
                call.args.len(),
                decl.name
            ))
        );
    }

    let result_at = skip_space(&text, decl.close + 1);
    anyhow::ensure!(
        text[result_at..].starts_with(&decl.results),
        "the primitive result token of `{}` cannot be located; nothing was written",
        decl.name
    );
    let mut rewritten = BTreeMap::new();
    if decl.results != requested_result {
        let replacement = splice(
            &text,
            &[(
                result_at,
                result_at + decl.results.len(),
                requested_result.to_string(),
            )],
        );
        anyhow::ensure!(
            comments(&text) == comments(&replacement),
            "{}: changing its result would drop or change a comment; nothing was written",
            display(root, file)
        );
        rewritten.insert(file.to_path_buf(), replacement);
    }
    let proposal: Vec<(PathBuf, String)> = if rewritten.is_empty() {
        vec![(file.to_path_buf(), text.clone())]
    } else {
        rewritten
            .iter()
            .map(|(path, source)| (path.clone(), source.clone()))
            .collect()
    };
    let compiler = crate::verify::compile_go_shadow(remote, root, file, &proposal)
        .await
        .context(
            "the complete Go result replacement could not be compiled in its private shadow; nothing was written",
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
                "{} changed while the result replacement was planned and compiled; nothing was written",
                display(root, path)
            );
        }
        if !rewritten.is_empty() {
            crate::refactor::apply_workspace_edit(
                root,
                &crate::signature::whole_file_edit(&rewritten),
            )?;
            applied = true;
        }
    }
    Ok(SignatureChange {
        symbol: decl.name,
        root: root.to_path_buf(),
        file: display(root, file),
        old_signature: normalize(&text[decl.open + 1..decl.close]),
        new_signature: normalize(&text[decl.open + 1..decl.close]),
        rule: String::new(),
        rewritten: rewritten
            .into_iter()
            .map(|(path, source)| (path.to_string_lossy().into_owned(), source))
            .collect(),
        unmatched: Vec::new(),
        unexpected: Vec::new(),
        diagnostics: Vec::new(),
        applied,
        returns: Some((decl.results, requested_result.to_string())),
        visibility: None,
        asyncness: None,
        not_async: Vec::new(),
    })
}

/// Result replacement has no call-site edits. Reordering, removing or adding a parameter would
/// make this a different operation and is refused before the compiler can normalize it away.
pub(crate) fn unchanged_parameters(declared: &[GoParam], request: &[Param]) -> Result<()> {
    anyhow::ensure!(
        request.len() == declared.len(),
        "{}",
        refusal(
            "a Go result replacement requires the existing named parameter list exactly unchanged"
                .to_string()
        )
    );
    for (expected, requested) in declared.iter().zip(request) {
        anyhow::ensure!(
            matches!(requested, Param::Keep(name) if name == &expected.name),
            "{}",
            refusal(format!(
                "a Go result replacement requires the existing named parameter list exactly unchanged; expected `{}`",
                expected.name
            ))
        );
    }
    Ok(())
}
