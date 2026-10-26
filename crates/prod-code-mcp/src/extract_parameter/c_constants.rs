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
    tokens::{Token, pairs, tokens},
};
use anyhow::{Context, Result};
use std::{net::SocketAddr, ops::Range, path::Path};

/// Required constant operands cannot start reading the new runtime parameter.
/// Skip complete declarations/conditions, including their conditional branches.
pub(super) fn constant_end(
    ts: &[Token<'_>],
    paired: &[Option<usize>],
    at: usize,
    syntax: Syntax,
) -> Option<usize> {
    let word = ts[at].text;
    if (matches!(word, "_Static_assert" | "_Alignas")
        || (syntax == Syntax::Cpp && matches!(word, "static_assert" | "alignas")))
        && ts.get(at + 1)?.text == "("
    {
        return paired[at + 1].map(|end| end + 1);
    }
    if syntax == Syntax::Cpp && word == "constexpr" {
        // `if constexpr` has a constant condition, but runtime branch bodies.
        if at > 0 && ts[at - 1].text == "if" {
            return (ts.get(at + 1)?.text == "(")
                .then(|| paired[at + 1].map(|end| end + 1))
                .flatten();
        }
        return Some(seek(ts, paired, at + 1, &[";", "}"]));
    }
    if word == "case" {
        let mut end = seek(ts, paired, at + 1, &[":", ";", "}"]);
        while ts.get(end).is_some_and(|t| t.text == ":")
            && super::regions::conditional_colon(ts, paired, end)
        {
            end = seek(ts, paired, end + 1, &[":", ";", "}"]);
        }
        return Some((end + 1).min(ts.len()));
    }
    None
}

/// In C11 `static_assert` may be an ordinary function or an assert.h macro;
/// `alignas` may be a function, a macro, or C++ grammar in a `.h` file.
/// Ask the analyzer about each invocation touched by the plan. A missing or
/// unfamiliar answer is incomplete coverage, never evidence of a constant macro.
/// Filter required storage/header constants before resolving ambiguous calls.
#[allow(clippy::too_many_arguments)]
pub(super) async fn assertions(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    syntax: Syntax,
    scope: Range<usize>,
    mut hits: Vec<Range<usize>>,
) -> Result<Vec<Range<usize>>> {
    if syntax != Syntax::C {
        return Ok(hits);
    }
    hits = super::c_declarations::preserve(remote, root, file, text, scope.clone(), hits).await?;
    let ts = tokens(&text[scope.clone()], syntax)?;
    let paired = pairs(&ts)?;
    for (i, token) in ts.iter().enumerate() {
        if !matches!(token.text, "static_assert" | "alignas")
            || !ts.get(i + 1).is_some_and(|t| t.text == "(")
        {
            continue;
        }
        let end = paired[i + 1].context("incomplete C assertion invocation")?;
        let call = scope.start + token.start..scope.start + ts[end].end;
        if !hits
            .iter()
            .any(|hit| hit.start < call.end && call.start < hit.end)
        {
            continue;
        }
        let (line, col) = crate::signature::position_at(text, call.start)?;
        let hover = crate::tools::execute_lsp_query(remote, root, file, "textDocument/hover", serde_json::json!({
            "textDocument": {"uri": url::Url::from_file_path(file).map_err(|_| anyhow::anyhow!("invalid source path"))?.to_string()},
            "position": {"line": line - 1, "character": col - 1},
        })).await.context("incomplete C assertion coverage; refused before writes")?;
        let value = hover
            .pointer("/contents/value")
            .or_else(|| hover.get("contents"))
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let heading = value
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .replace('`', "");
        if heading == format!("### function {}", token.text) {
            continue;
        }
        // A callable variable has a variable hover, not a function heading.
        // Resolve the complete invocation; a Call with this callee is runtime
        // evidence, whereas an assertion/macro declaration is not.
        let ast = super::c_declarations::ast(remote, root, file, text, call.clone()).await?;
        if ast["role"] == "expression"
            && ast["kind"] == "Call"
            && ast["children"]
                .as_array()
                .and_then(|children| children.first())
                .is_some_and(|callee| resolved_callee(callee, token.text))
        {
            continue;
        }
        if token.text == "alignas" {
            anyhow::bail!(
                "incomplete C/C++ alignment coverage at {line}:{col}: expected a resolved runtime callable; possible constant operand in a header or macro; refused before writes"
            );
        }
        let macro_body = value.lines().find_map(|line| {
            line.trim()
                .strip_prefix('#')?
                .trim_start()
                .strip_prefix("define ")
        });
        let assertion_macro = if let Some(body) = macro_body {
            let definition = tokens(body, syntax)?;
            let paired = pairs(&definition)?;
            let replacement = if definition.get(1).is_some_and(|t| t.text == "(") {
                paired[1].map(|end| end + 1)
            } else {
                Some(1)
            };
            definition
                .first()
                .is_some_and(|t| t.text == "static_assert")
                && replacement
                    .and_then(|at| definition.get(at))
                    .is_some_and(|t| t.text == "_Static_assert")
        } else {
            false
        };
        anyhow::ensure!(
            assertion_macro,
            "incomplete C assertion coverage for static_assert at {line}:{col}: expected a resolved runtime callable or _Static_assert macro; refused before writes"
        );
        hits.retain(|hit| hit.end <= call.start || call.end <= hit.start);
    }
    Ok(hits)
}

pub(super) fn resolved_callee(node: &serde_json::Value, name: &str) -> bool {
    (node["role"] == "expression" && node["kind"] == "DeclRef" && node["detail"] == name)
        || (node["kind"] == "ImplicitCast"
            && node["children"]
                .as_array()
                .is_some_and(|children| children.iter().any(|child| resolved_callee(child, name))))
}
