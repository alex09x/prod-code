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
use std::net::SocketAddr;
use std::path::Path;

use super::duplicates::{collect_copies, process_and_validate_duplicates};
use super::rewrite::{indent_at, literal_type, rename_placeholder, rewrite_of, with_arguments};
use super::tokens::{is_ident, mentions, tokens};
use super::types::{Extracted, PLACEHOLDER};

/// Extracts the selection `line`:`col` .. `end_line`:`end_col` of `file` into `fn name`, and,
/// with `duplicates`, replaces every other place in the file that has the same text with the
/// same call, where the result type-checks. Nothing is written; see [`Extracted::write`].
#[allow(clippy::too_many_arguments)]
pub async fn extract_function(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    (line, col): (u32, u32),
    (end_line, end_col): (u32, u32),
    name: &str,
    duplicates: bool,
    parameterize: bool,
    other_files: bool,
) -> Result<Extracted> {
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext != "rs" {
        return crate::extract_function_polyglot::extract_function_polyglot(
            remote,
            root,
            file,
            (line, col),
            (end_line, end_col),
            name,
            duplicates,
            parameterize,
            other_files,
        )
        .await;
    }
    anyhow::ensure!(
        !name.is_empty()
            && name.chars().all(is_ident)
            && !name.starts_with(|c: char| c.is_ascii_digit()),
        "`{name}` is not an identifier"
    );
    let text =
        std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    anyhow::ensure!(
        !mentions(&text, PLACEHOLDER),
        "the file already uses `{PLACEHOLDER}`, the name rust-analyzer gives the function it \
         extracts; rename that first"
    );
    anyhow::ensure!(
        !mentions(&text, name),
        "the file already has something called `{name}`; choose another name"
    );
    let start =
        crate::signature::offset_of(&text, line, col).context("the start is not in the file")?;
    let end = crate::signature::offset_of(&text, end_line, end_col)
        .context("the end is not in the file")?;
    anyhow::ensure!(start < end, "the selection is empty");

    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let edit = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "prodCode/applyAssist",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "range": {
                "start": { "line": line - 1, "character": col - 1 },
                "end": { "line": end_line - 1, "character": end_col - 1 }
            },
            "id": "extract_function",
        }),
    )
    .await
    .context("rust-analyzer cannot extract a function from this selection")?;
    let (planned, _) = crate::refactor::planned_texts(root, &edit)?;
    let canonical = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    let extracted = planned
        .into_iter()
        .find(|(p, _)| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()) == canonical)
        .map(|(_, t)| t)
        .context("the assist did not rewrite the file")?;

    let rewrite = rewrite_of(&text, &extracted, start, end);
    let call = rewrite.as_ref().map(|r| r.call.clone());
    let call_indent = indent_at(&text, start).to_string();
    let selection = text[start..end].trim().to_string();
    let selection_at = start + (text[start..end].len() - text[start..end].trim_start().len());
    let sel_tokens = tokens(&selection);

    let copies = collect_copies(
        root,
        file,
        &text,
        &selection,
        start,
        end,
        duplicates,
        parameterize,
        other_files,
    );

    let mut varying: Vec<usize> = copies
        .iter()
        .flat_map(|(_, _, c)| c.differs.iter().map(|(k, _)| *k))
        .collect();
    varying.sort_unstable();
    varying.dedup();
    let mut parameters: Vec<(String, String)> = Vec::new();
    let mut untyped: Option<String> = None;
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    for (n, k) in varying.iter().enumerate() {
        let (_, ts, te) = sel_tokens[*k];
        let literal = &selection[ts..te];
        let (l, c) = crate::signature::position_at(&text, selection_at + ts)?;
        let hover = crate::tools::execute_lsp_query(
            remote,
            root,
            file,
            "textDocument/hover",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "position": { "line": l - 1, "character": c - 1 },
            }),
        )
        .await
        .ok()
        .and_then(|h| {
            h.pointer("/contents/value")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_default();
        let Some(ty) = literal_type(&hover) else {
            untyped = Some(literal.to_string());
            break;
        };
        let name = if varying.len() == 1 {
            "value".to_string()
        } else {
            format!("value{}", n + 1)
        };
        parameters.push((name, ty));
    }
    if untyped.is_some() {
        parameters.clear();
    }

    let function = rewrite.as_ref().map(|r| {
        let at = start + r.call.len() + (r.inserted_at - end);
        (at, at + r.function_len)
    });
    let mut base_edits: Vec<(usize, usize, String)> = Vec::new();
    let mut parameterized = parameters.is_empty();
    if let (Some((fs, fe)), false) = (function, parameters.is_empty()) {
        let body = &extracted[fs..fe];
        let def = body.find(&format!("fn {PLACEHOLDER}")).map(|i| fs + i + 3);
        let list = def.and_then(|d| crate::signature::param_span(&extracted, d));
        let window = tokens(body).windows(sel_tokens.len()).position(|w| {
            w.iter()
                .zip(&sel_tokens)
                .all(|(a, b)| body[a.1..a.2] == selection[b.1..b.2])
        });
        if let (Some((_, open, close)), Some(w)) = (list, window) {
            let body_tokens = tokens(body);
            let declared = parameters
                .iter()
                .map(|(n, ty)| format!("{n}: {ty}"))
                .collect::<Vec<_>>()
                .join(", ");
            let sep = if extracted[open..close].trim().is_empty() {
                ""
            } else {
                ", "
            };
            base_edits.push((close, close, format!("{sep}{declared}")));
            for (n, k) in varying.iter().enumerate() {
                let (_, bs, be) = body_tokens[w + k];
                base_edits.push((fs + bs, fs + be, parameters[n].0.clone()));
            }
            parameterized = true;
        }
    }

    let own_literals: Vec<String> = varying
        .iter()
        .map(|k| selection[sel_tokens[*k].1..sel_tokens[*k].2].to_string())
        .collect();
    if let (Some(c), false) = (call.as_deref(), parameters.is_empty())
        && let Some(own) = with_arguments(c, &own_literals)
    {
        base_edits.push((start, start + c.len(), own));
    }

    let (found, result, diagnostics, final_call) = process_and_validate_duplicates(
        remote,
        root,
        file,
        name,
        &extracted,
        &selection,
        &sel_tokens,
        start,
        end,
        rewrite.as_ref(),
        call,
        &call_indent,
        &copies,
        &varying,
        &own_literals,
        &mut parameters,
        untyped.as_deref(),
        parameterized,
        &base_edits,
        other_files,
        function,
    )
    .await?;

    Ok(Extracted {
        name: name.to_string(),
        root: root.to_path_buf(),
        file: file
            .strip_prefix(root)
            .unwrap_or(file)
            .to_string_lossy()
            .into_owned(),
        call: rename_placeholder(final_call.as_deref().unwrap_or(&text[start..end]), name),
        parameters: parameters
            .iter()
            .map(|(n, ty)| format!("{n}: {ty}"))
            .collect(),
        duplicates: found,
        rewritten: result
            .into_iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t))
            .collect(),
        diagnostics,
        applied: false,
    })
}
