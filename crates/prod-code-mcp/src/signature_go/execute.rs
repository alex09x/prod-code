/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature::{Modifiers, Param, SignatureChange};
use crate::signature_go::add_params::add_parameters;
use crate::signature_go::parse::{declarations, parameters};
use crate::signature_go::reorder::reorder_or_remove_parameters;
use crate::signature_go::result::replace_result;
use crate::signature_go::text::{display, offset_at};
use crate::signature_go::types::{refusal, refuse_non_result_modifiers};
use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::Path;

/// Reorders or removes named parameters of the Go function or method at `file:line:col` (1-based; the
/// position may be anywhere from its `func` keyword to the `)` closing its parameters).
///
/// `request` names each retained parameter once; omitted parameters must be provably unused.
/// `modifiers` must be empty.
/// `force` is accepted for the common dispatch and overrides no refusal.
#[allow(clippy::too_many_arguments)]
pub async fn change_with(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
    request: &[Param],
    modifiers: &Modifiers,
    apply: bool,
    force: bool,
) -> Result<SignatureChange> {
    let _ = force;
    anyhow::ensure!(
        file.extension().is_some_and(|e| e == "go"),
        "{} is not a Go file",
        file.display()
    );
    refuse_non_result_modifiers(modifiers)?;
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    anyhow::ensure!(
        line > 0 && col > 0,
        "{}:{line}:{col} is not in the file",
        file.display()
    );
    let at = offset_at(&text, line - 1, col - 1)
        .with_context(|| format!("{}:{line}:{col} is not in the file", file.display()))?;
    let decl = declarations(&text)
        .into_iter()
        .find(|d| d.func_at <= at && at <= d.close)
        .with_context(|| {
            refusal(format!(
                "{}:{line}:{col} is not in the header of a declared Go function or method (a \
                 function literal, a function type or an interface method has no declaration \
                 gopls can change)",
                display(root, file)
            ))
        })?;
    let declared = parameters(&text[decl.open + 1..decl.close]).map_err(|why| {
        refusal(format!(
            "`{}` cannot be reordered by name: {why}; name every parameter first",
            decl.name
        ))
    })?;
    if let Some(result) = modifiers.returns.as_deref() {
        return replace_result(
            remote, root, file, text, decl, declared, request, result, apply,
        )
        .await;
    }
    if request.iter().any(|p| matches!(p, Param::Add { .. })) {
        return add_parameters(remote, root, file, text, decl, declared, request, apply).await;
    }
    reorder_or_remove_parameters(remote, root, file, text, decl, declared, request, apply).await
}
