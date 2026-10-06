/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::signature_go::edits::{ensure_inside, same_file};
use crate::signature_go::interface::receiver_selector_call;
use crate::signature_go::interface::{declares_interface_method, interface_method_file};
use crate::signature_go::parse::{call_parens, ident_at};
use crate::signature_go::text::{display, line_col_utf16, offset_at, split_list, strip_comments};
use crate::signature_go::types::{Call, Decl};
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// A receiver method may satisfy an imported interface even when no source interface declaration
/// or interface-typed call names it. gopls reports those relations from the concrete method, so
/// an empty result is the proof that extending this method does not alter an interface contract.
pub(crate) async fn receiver_interface_evidence(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    declaration_text: &str,
    name: &str,
    name_at: usize,
) -> Result<()> {
    let (line, character) = line_col_utf16(declaration_text, name_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/implementation",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line, "character": character },
        }),
    )
    .await
    .context("gopls could not list the method's interface implementations")?;
    let implementations = crate::refactor::lsp_locations(&answer, "interface implementations")?;
    anyhow::ensure!(
        implementations.is_empty(),
        "`{}` has interface implementation evidence at {}; interface dispatch cannot be reconciled",
        name,
        implementations
            .iter()
            .map(|(path, line, column)| format!("{}:{line}:{column}", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/// gopls's complete reference answer for the function. The declaration must occur exactly once;
/// every other location must be current, inside the checkout and a direct call.
pub(crate) async fn function_reference_evidence(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    canonical_root: &Path,
    declaration_text: &str,
    decl: &Decl,
    receiver_type: Option<&str>,
) -> Result<(BTreeMap<PathBuf, String>, Vec<Call>)> {
    let (line, character) = line_col_utf16(declaration_text, decl.name_at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    });
    let mut answer = serde_json::Value::Null;
    for attempt in 0..=crate::impact::COLD_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
        }
        answer = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
        .await
        .context("gopls could not list the function's references")?;
        if answer.as_array().is_some_and(|entries| !entries.is_empty()) {
            break;
        }
    }
    let entries = answer
        .as_array()
        .filter(|entries| !entries.is_empty())
        .with_context(|| {
            format!(
                "gopls listed no location, not even the declaration of `{}`: {answer}",
                decl.name
            )
        })?;
    let mut originals = BTreeMap::new();
    originals.insert(file.to_path_buf(), declaration_text.to_string());
    let mut calls = Vec::new();
    let mut declarations = 0usize;
    let mut seen = BTreeSet::new();
    for entry in entries {
        let number = |end: &str, key: &str| {
            entry
                .pointer(&format!("/range/{end}/{key}"))
                .and_then(|value| value.as_u64())
                .and_then(|value| u32::try_from(value).ok())
                .filter(|value| *value < u32::MAX)
        };
        let (Some(uri), Some(line), Some(column), Some(end_line), Some(end_column)) = (
            entry.get("uri").and_then(|value| value.as_str()),
            number("start", "line"),
            number("start", "character"),
            number("end", "line"),
            number("end", "character"),
        ) else {
            anyhow::bail!("a function reference is malformed: {entry}");
        };
        let uri = url::Url::parse(uri).context("a function reference has an invalid URI")?;
        anyhow::ensure!(
            uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
            "a function reference is not a plain file URI: {uri}"
        );
        let reported = uri
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("a function reference is not a local file URI: {uri}"))?;
        ensure_inside(canonical_root, &reported)?;
        let path = originals
            .keys()
            .find(|known| same_file(known, &reported))
            .cloned()
            .unwrap_or(reported);
        if !originals.contains_key(&path) {
            originals.insert(
                path.clone(),
                std::fs::read_to_string(&path).with_context(|| {
                    format!("cannot read {}; nothing was written", path.display())
                })?,
            );
        }
        let source = &originals[&path];
        let offset = offset_at(source, line, column)
            .filter(|at| ident_at(source, *at) == Some(decl.name.as_str()))
            .with_context(|| {
                format!(
                    "{}:{}:{} is not on `{}`; the reference is stale",
                    display(root, &path),
                    line + 1,
                    column + 1,
                    decl.name
                )
            })?;
        anyhow::ensure!(
            offset_at(source, end_line, end_column) == offset.checked_add(decl.name.len()),
            "{}:{}:{} does not span exactly `{}`; the reference is stale or malformed",
            display(root, &path),
            line + 1,
            column + 1,
            decl.name
        );
        anyhow::ensure!(
            seen.insert((path.clone(), offset)),
            "{}:{}:{} is listed more than once",
            display(root, &path),
            line + 1,
            column + 1
        );
        if same_file(&path, file) && offset == decl.name_at {
            declarations += 1;
            continue;
        }
        let at = format!("{}:{}:{}", display(root, &path), line + 1, column + 1);
        if let Some(receiver_type) = receiver_type {
            anyhow::ensure!(
                !declares_interface_method(source, &decl.name),
                "`{}` has an interface declaration in {}; interface dispatch cannot be reconciled",
                decl.name,
                display(root, &path)
            );
            receiver_selector_call(source, offset, receiver_type).with_context(|| {
                format!(
                    "`{}` is not a direct selector call at {at}; interface dispatch, method \
                     values and method expressions are not supported",
                    decl.name
                )
            })?;
        }
        let (open, close) = call_parens(source, offset).with_context(|| {
            format!(
                "`{}` is used as a value or another unsupported shape at {at}",
                decl.name
            )
        })?;
        calls.push(Call {
            path,
            at,
            open,
            close,
            args: split_list(&strip_comments(&source[open + 1..close])),
        });
    }
    anyhow::ensure!(
        declarations == 1,
        "gopls listed the declaration of `{}` {declarations} times instead of exactly once",
        decl.name
    );
    if receiver_type.is_some()
        && let Some(path) = interface_method_file(canonical_root, &decl.name)?
    {
        anyhow::bail!(
            "`{}` has an interface declaration in {}; interface dispatch cannot be reconciled",
            decl.name,
            display(root, &path)
        );
    }
    Ok((originals, calls))
}

/// The uses gopls knows of the parameter declared at `at`, as offsets in the body. The question
/// includes the declaration, so that an answer from a server that has not loaded the package
/// (nothing at all) is told apart from "no use": that one is asked again a few times, then
/// refused.
pub(crate) async fn parameter_uses(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    name: &str,
    at: usize,
    body: (usize, usize),
) -> Result<Vec<usize>> {
    let (line, character) = line_col_utf16(text, at);
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line, "character": character },
        "context": { "includeDeclaration": true },
    });
    let mut answer = serde_json::Value::Null;
    for attempt in 0..=crate::impact::COLD_RETRIES {
        if attempt > 0 {
            tokio::time::sleep(crate::impact::COLD_WAIT).await;
        }
        answer = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/references",
            params.clone(),
        )
        .await
        .context("the references could not be listed")?;
        if answer.as_array().is_some_and(|a| !a.is_empty()) {
            break;
        }
    }
    parameter_evidence(&answer, file, text, name, at, body)
}

/// The uses in a `textDocument/references` answer for the parameter `name` declared at `at`,
/// which must be well-formed and complete to count: a list of locations in `file`, each on the
/// name as the file reads now, the declaration among them, and every other one inside the body.
/// Anything else is an error, never "unused": a stale position, a location elsewhere, a missing
/// declaration or an empty answer says the server was not answering about this parameter.
pub(crate) fn parameter_evidence(
    answer: &serde_json::Value,
    file: &Path,
    text: &str,
    name: &str,
    at: usize,
    body: (usize, usize),
) -> Result<Vec<usize>> {
    let entries = answer
        .as_array()
        .filter(|a| !a.is_empty())
        .with_context(|| {
            format!("the answer lists no location, not even the declaration: {answer}")
        })?;
    let mut declared = false;
    let mut uses = Vec::new();
    for entry in entries {
        let number = |end: &str, key: &str| {
            entry
                .pointer(&format!("/range/{end}/{key}"))
                .and_then(|v| v.as_u64())
                .and_then(|v| u32::try_from(v).ok())
                .filter(|v| *v < u32::MAX)
        };
        let (Some(uri), Some(l), Some(c), Some(el), Some(ec)) = (
            entry.get("uri").and_then(|u| u.as_str()),
            number("start", "line"),
            number("start", "character"),
            number("end", "line"),
            number("end", "character"),
        ) else {
            anyhow::bail!("a location is malformed: {entry}");
        };
        let uri = url::Url::parse(uri).context("a parameter location has an invalid URI")?;
        anyhow::ensure!(
            uri.scheme() == "file" && uri.query().is_none() && uri.fragment().is_none(),
            "a parameter location is not a plain file URI: {uri}"
        );
        let path = uri
            .to_file_path()
            .map_err(|_| anyhow::anyhow!("a parameter location is not a local file URI: {uri}"))?;
        anyhow::ensure!(
            same_file(&path, file),
            "a use in {} is outside the function; the answer is not about this parameter",
            path.display()
        );
        let offset = offset_at(text, l, c)
            .filter(|&o| ident_at(text, o) == Some(name))
            .with_context(|| {
                format!(
                    "the location {}:{} is not on `{name}`; the file may have changed since it \
                     was read",
                    l + 1,
                    c + 1
                )
            })?;
        anyhow::ensure!(
            offset_at(text, el, ec) == offset.checked_add(name.len()),
            "the location {}:{} does not span exactly `{name}`; its range is stale or malformed",
            l + 1,
            c + 1
        );
        if offset == at {
            declared = true;
        } else {
            anyhow::ensure!(
                body.0 < offset && offset < body.1,
                "the location {}:{} is outside the function's body",
                l + 1,
                c + 1
            );
            uses.push(offset);
        }
    }
    anyhow::ensure!(
        declared,
        "the answer does not include the parameter's own declaration, so it is not about it"
    );
    Ok(uses)
}
