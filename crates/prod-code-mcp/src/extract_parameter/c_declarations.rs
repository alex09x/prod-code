/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    Syntax,
    contexts::seek,
    tokens::{pairs, tokens},
};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::{net::SocketAddr, ops::Range, path::Path};

/// C static storage needs constant initializers. In C-classified headers,
/// constexpr is ambiguous with ordinary C names, and static may be C++ runtime
/// initialization. Resolve those boundaries before accepting any edits.
pub(super) async fn preserve(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    scope: Range<usize>,
    mut hits: Vec<Range<usize>>,
) -> Result<Vec<Range<usize>>> {
    let ts = tokens(&text[scope.clone()], Syntax::C)?;
    let paired = pairs(&ts)?;
    // The scope starts at the parameter-list opener. Parameter declarations
    // (including C array parameters with `static`) are outside the body.
    let start = paired.first().copied().flatten().map_or(0, |end| end + 1);
    for i in start..ts.len() {
        let word = ts[i].text;
        if !matches!(word, "static" | "constexpr") {
            continue;
        }
        let condition = word == "constexpr" && i > 0 && ts[i - 1].text == "if";
        let end = if condition && ts.get(i + 1).is_some_and(|t| t.text == "(") {
            paired[i + 1].context("incomplete constexpr condition")? + 1
        } else {
            seek(&ts, &paired, i + 1, &[";", "}"])
        };
        let range = scope.start + ts[i].start..scope.start + ts[end.saturating_sub(1)].end;
        if !hits.iter().any(|hit| overlaps(hit, &range)) {
            continue;
        }
        if word == "static" && file.extension().is_some_and(|ext| ext == "c") {
            hits.retain(|hit| !overlaps(hit, &range));
            continue;
        }
        let (line, col) = crate::signature::position_at(text, range.start)?;
        if word == "constexpr" {
            // A resolved C variable/function/typedef with this spelling is a
            // runtime name, not a C++ specifier. Missing identities prove nothing.
            let answer = crate::tools::execute_lsp_query(
                remote,
                root,
                file,
                "textDocument/definition",
                json!({
                    "textDocument": {"uri": uri(file)?},
                    "position": {"line":line-1,"character":col-1},
                }),
            )
            .await
            .context("incomplete C/C++ constexpr coverage; refused before writes")?;
            let declarations = crate::refactor::lsp_locations(&answer, "constexpr binding")?;
            if declarations.len() == 1 {
                continue;
            }
            anyhow::ensure!(
                declarations.is_empty(),
                "incomplete C/C++ constexpr coverage at {line}:{col}: ambiguous declaration; refused before writes"
            );
        }
        // clangd prints cpp code fences for both C and C++, and both static
        // Var ASTs are identical. Neither is proof of header compilation mode.
        let node = ast(remote, root, file, text, range.clone()).await?;
        anyhow::ensure!(
            word != "static" || constexpr(&node, false),
            "incomplete C/C++ static initializer coverage at {line}:{col}: unknown header compilation language; refused before writes"
        );
        anyhow::ensure!(
            constexpr(&node, condition),
            "incomplete C/C++ constexpr coverage at {line}:{col}: expected a resolved name or constexpr declaration; refused before writes"
        );
        hits.retain(|hit| !overlaps(hit, &range));
    }
    Ok(hits)
}

fn overlaps(left: &Range<usize>, right: &Range<usize>) -> bool {
    left.start < right.end && right.start < left.end
}

fn constexpr(node: &Value, condition: bool) -> bool {
    let kind = if condition { "If" } else { "Var" };
    let role = if condition {
        "statement"
    } else {
        "declaration"
    };
    // clangd's Var metadata follows the quoted type, after the declaration
    // name. A C variable/type named constexpr must not satisfy this predicate.
    (node["kind"] == kind
        && node["role"] == role
        && node["arcana"].as_str().is_some_and(|arcana| {
            arcana
                .lines()
                .next()
                .unwrap_or_default()
                .rsplit('\'')
                .next()
                .unwrap_or_default()
                .split_whitespace()
                .any(|word| word == "constexpr")
        }))
        || (node["kind"] == "Decl"
            && node["children"].as_array().is_some_and(|children| {
                !children.is_empty() && children.iter().all(|child| constexpr(child, condition))
            }))
}

pub(super) async fn ast(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    range: Range<usize>,
) -> Result<Value> {
    let (line, col) = crate::signature::position_at(text, range.start)?;
    let (end_line, end_col) = crate::signature::position_at(text, range.end)?;
    crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/ast",
        json!({
            "textDocument":{"uri":uri(file)?},
            "range":{"start":{"line":line-1,"character":col-1},
                     "end":{"line":end_line-1,"character":end_col-1}},
        }),
    )
    .await
    .context("incomplete C/C++ constant/call coverage; refused before writes")
}

fn uri(file: &Path) -> Result<String> {
    url::Url::from_file_path(file)
        .map(|uri| uri.to_string())
        .map_err(|_| anyhow::anyhow!("invalid source path"))
}
