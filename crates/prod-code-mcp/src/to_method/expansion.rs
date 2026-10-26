/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Parse expanded types in the analyzer's validation overlay. A plain expression
//! path uses the value namespace; qualified expression paths may name a type's
//! associated constant and must still participate in the identity guard.

use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::json;
use std::{net::SocketAddr, ops::Range, path::Path};

use crate::fixture::builder::lexer::{Kind, Source};

const PREFIX: &str = "type __ProdReceiver = ";

#[derive(Deserialize)]
struct Node {
    kind: String,
    start: [usize; 3],
    end: [usize; 3],
    #[serde(default)]
    children: Vec<Node>,
}

pub(super) async fn value_names(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    expanded: &Source<'_>,
) -> Result<Vec<Range<usize>>> {
    let text = format!("{PREFIX}{};", expanded.text);
    // Reuse the proposed-text facility, on its isolated validation engine. No
    // physical source edit or change to the ordinary semantic session is needed.
    let mut session =
        crate::session::LspSession::open_for_validation(remote, root, Some(file)).await?;
    let uri = session.uri_for(file)?;
    let answer = session
        .query_with_text(
            file,
            &text,
            "rust-analyzer/viewSyntaxTree",
            json!({"textDocument": {"uri": uri}}),
        )
        .await;
    session.close().await;
    let answer = answer.context("cannot establish expanded receiver namespaces")?;
    let tree: Node = serde_json::from_str(
        answer
            .as_str()
            .context("cannot establish expanded receiver namespaces without Rust syntax")?,
    )
    .context("unknown expanded receiver syntax format")?;
    ensure!(
        tree.kind == "SOURCE_FILE" && tree.start[0] == 0 && tree.end[0] == text.len(),
        "expanded receiver syntax does not cover the proposed type"
    );
    let mut spans = Vec::new();
    collect(&tree, &text, 0, &mut spans)?;
    Ok(spans
        .into_iter()
        .filter_map(|span| {
            let start = span.start.checked_sub(PREFIX.len())?;
            let end = span.end.checked_sub(PREFIX.len())?;
            if end > expanded.text.len() {
                return None;
            }
            let from = expanded.tokens.partition_point(|t| t.end <= start);
            let to = expanded.tokens.partition_point(|t| t.start < end);
            Some(from..to)
        })
        .collect())
}

fn collect(node: &Node, text: &str, depth: usize, values: &mut Vec<Range<usize>>) -> Result<()> {
    let span = node.start[0]..node.end[0];
    ensure!(
        depth < 256 && span.start <= span.end && text.get(span.clone()).is_some(),
        "invalid expanded receiver syntax span"
    );
    ensure!(node.kind != "ERROR", "incomplete expanded receiver syntax");
    if node.kind == "PATH_EXPR" {
        let path = Source::new(&text[span.clone()])?;
        // Do not exempt U::N, <U as Trait>::N, U::<T>, or type arguments
        // nested inside expression paths. Only a bare value name is proven here.
        if path.tokens.len() == 1 && path.tokens[0].kind == Kind::Ident {
            values.push(span.clone());
        }
    }
    let opaque = matches!(
        node.kind.as_str(),
        "STRING" | "BYTE_STRING" | "C_STRING" | "CHAR" | "BYTE" | "COMMENT"
    );
    if !opaque {
        let mut at = span.start;
        for child in &node.children {
            ensure!(
                child.start[0] == at && child.end[0] <= span.end,
                "noncontiguous expanded receiver syntax"
            );
            if node.kind == "IDENT_PAT" && child.kind == "NAME" {
                // A pattern declares a value; it cannot capture a method type.
                values.push(child.start[0]..child.end[0]);
            }
            collect(child, text, depth + 1, values)?;
            at = child.end[0];
        }
        ensure!(
            node.children.is_empty() || at == span.end,
            "incomplete expanded receiver syntax"
        );
    }
    Ok(())
}
