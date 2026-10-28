/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::{net::SocketAddr, path::Path};

use anyhow::{Context, Result};
use ra_ap_syntax::Edition;

use super::budget::{MoveBudget, document_symbols};
use super::item::{append_item, owner_token_pos, resolve_type_def, verified_target_impl_index};
use super::ownership::{Graph, Owner};
use super::rust_syntax::impl_header_for_owner;

pub(super) struct PreparedTargetImpl {
    pub index: usize,
    pub text: String,
}

/// Reuses one analyzer-verified target impl or safely creates a qualified one for the same type.
#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare_target_impl(
    budget: &MoveBudget,
    remote: SocketAddr,
    root: &Path,
    source_file: &Path,
    source_text: &str,
    impl_span: (usize, usize),
    target: &Path,
    target_text: &str,
    owner: &str,
    target_is_new: bool,
    graph: &Graph,
    target_owners: &[Owner],
    edition: Edition,
) -> Result<PreparedTargetImpl> {
    if !target_is_new
        && let Some(index) = budget
            .run("resolving the target impl", async {
                Ok(verified_target_impl_index(
                    remote,
                    root,
                    source_file,
                    source_text,
                    impl_span,
                    target,
                    target_text,
                    owner,
                )
                .await)
            })
            .await?
    {
        return Ok(PreparedTargetImpl {
            index,
            text: target_text.to_string(),
        });
    }

    let has_unverified_impl = crate::extract_field::impl_blocks(target_text)
        .into_iter()
        .any(|(target_owner, at, open, _)| {
            target_owner == owner && !target_text[at..open].contains(" for ")
        });
    anyhow::ensure!(
        !has_unverified_impl,
        "target contains an inherent impl for `{owner}` whose type identity cannot be verified; nothing was written"
    );

    let owner_pos = owner_token_pos(source_text, impl_span.0, impl_span.1, owner)
        .context("cannot locate the inherent type; nothing was written")?;
    let (owner_uri, _, _) = budget
        .run("resolving the inherent owner type", async {
            Ok(resolve_type_def(remote, root, source_file, owner_pos).await)
        })
        .await?
        .context("cannot resolve the inherent type; nothing was written")?;
    let owner_def_path = url::Url::parse(&owner_uri)?
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("resolved inherent type `{owner}` is not a local file"))?;
    let owner_owners = graph.owners(&owner_def_path, None)?;
    anyhow::ensure!(
        owner_owners.iter().any(|owner| {
            target_owners
                .iter()
                .any(|target_owner| owner.identity == target_owner.identity)
        }),
        "inherent type `{owner}` and target are not in the same Cargo target; nothing was written"
    );
    let owner_module = graph.caller_spelling(target_owners, &owner_owners)?;
    let qualified_owner = format!("{owner_module}::{owner}");

    if target.exists() && owner_def_path != target {
        let symbols = budget
            .run(
                "checking target type names",
                document_symbols(remote, root, target),
            )
            .await?;
        let shadows_owner = symbols.as_array().is_some_and(|symbols| {
            symbols.iter().any(|symbol| {
                symbol.get("name").and_then(serde_json::Value::as_str) == Some(owner)
                    && symbol
                        .get("kind")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|kind| matches!(kind, 2 | 3 | 5 | 10 | 11 | 23 | 26))
            })
        });
        anyhow::ensure!(
            !shadows_owner,
            "target declares a type named `{owner}`; its identity cannot be verified, so nothing was written"
        );
    }

    let header = impl_header_for_owner(source_text, impl_span, owner, &qualified_owner, edition)
        .context("cannot qualify the inherent impl owner; nothing was written")?;
    let block = format!("{header}\n}}");
    let text = if target_text.trim().is_empty() {
        format!("{block}\n")
    } else {
        append_item(target_text, &block)
    };
    let index = crate::extract_field::impl_blocks(&text)
        .len()
        .checked_sub(1)
        .ok_or_else(|| anyhow::anyhow!("could not construct target impl for `{owner}`"))?;
    Ok(PreparedTargetImpl { index, text })
}
