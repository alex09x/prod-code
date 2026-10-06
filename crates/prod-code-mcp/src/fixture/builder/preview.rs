/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result, bail};

use super::lexer::bare;
use super::outline::outline_node;
use super::plan::plan;
use super::types::{BuilderPreview, BuilderRequest, Verification};
use super::verify::verify;

/// Generates the builder for `request.symbol` and, when asked, checks it with the analyzer.
/// Nothing is written: the result carries the code and the file as it would be.
pub async fn preview(
    remote: SocketAddr,
    root: &Path,
    request: &BuilderRequest<'_>,
) -> Result<BuilderPreview> {
    let hit = crate::fixture::resolve_type(remote, root, request.symbol, request.hint).await?;
    let file = hit
        .path
        .strip_prefix(root)
        .unwrap_or(&hit.path)
        .to_string_lossy()
        .into_owned();
    let uri = url::Url::from_file_path(&hit.path)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", hit.path))?
        .to_string();
    let outline = crate::tools::execute_lsp_query(
        remote,
        root,
        &hit.path,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await?;
    let node = outline_node(&outline, request.symbol, hit.line)
        .with_context(|| {
            format!(
                "the analyzer's outline of {file} is malformed; refusing rather than guessing where `{}` is and which fields it has",
                request.symbol
            )
        })?
        .with_context(|| {
            format!(
                "the analyzer's outline of {file} has no declaration of `{}` at line {}",
                request.symbol, hit.line
            )
        })?;
    let text = std::fs::read_to_string(&hit.path)
        .with_context(|| format!("cannot read {}", hit.path.display()))?;
    let mut plan = plan(
        &text,
        request.symbol,
        (node.start, node.end),
        request.builder_name,
    )
    .with_context(|| format!("no builder for `{}` in {file}", request.symbol))?;
    match &node.fields {
        Some(listed) => {
            let read: Vec<&str> = plan.fields.iter().map(|f| bare(&f.name)).collect();
            let listed: Vec<&str> = listed.iter().map(|f| bare(f)).collect();
            if read != listed {
                bail!(
                    "the analyzer's outline of `{}` lists the fields [{}] but the declaration in {file} spells [{}]; refusing rather than generating a builder for part of the struct",
                    request.symbol,
                    listed.join(", "),
                    read.join(", ")
                );
            }
        }
        None if !plan.fields.is_empty() => plan.notes.push(
            "the analyzer's outline lists no fields for the struct; they were read from its declaration"
                .to_string(),
        ),
        None => {}
    }
    for name in [plan.builder_name.clone(), plan.error_name.clone()] {
        let hits =
            crate::tools::workspace_symbol_search(remote, root, &name, Some(&hit.path), 64).await?;
        if let Some(taken) = hits.iter().find(|h| bare(&h.name) == name) {
            bail!(
                "`{name}` is already declared in this workspace ({} at {}:{}); the builder would collide with it or change what an import of it names. Pass another `builder_name`",
                taken.kind,
                taken
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&taken.path)
                    .to_string_lossy(),
                taken.line
            );
        }
    }
    let verification = if request.verify {
        verify(remote, root, &hit.path, &file, &text, &plan).await?
    } else {
        Verification::Unverified {
            reason: "verification was not requested; the names were checked against the declaring file and the workspace index only".to_string(),
        }
    };
    Ok(BuilderPreview {
        file,
        plan,
        verification,
    })
}
