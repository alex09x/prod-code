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
    c_templates::{node_span, span},
    tokens::{pairs, tokens},
};
use anyhow::{Context, Result};
use std::{net::SocketAddr, ops::Range, path::Path};

/// A C++ array declarator requires constant bounds, whereas a subscript reads
/// runtime values. Query the bracket rather than the literal: clangd exposes
/// the enclosing array type there, including typedefs and pointer declarators.
/// C permits runtime VLA bounds, but initializer designators require constants.
pub(super) async fn preserve(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    syntax: Syntax,
    scope: Range<usize>,
    mut hits: Vec<Range<usize>>,
) -> Result<Vec<Range<usize>>> {
    let c_source = syntax == Syntax::C && file.extension().is_some_and(|ext| ext == "c");
    let ts = tokens(&text[scope.clone()], syntax)?;
    let paired = pairs(&ts)?;
    for (i, token) in ts.iter().enumerate().filter(|(_, t)| t.text == "[") {
        // A C designator starts an initializer element, follows another array
        // designator, or follows a field designator. Ordinary C declarators and
        // subscripts retain their existing runtime/VLA behavior. Tokens only
        // select queries; clangd must prove the required constant operand.
        let designator_start =
            i > 0 && matches!(ts[i - 1].text, "{" | "," | "]") || i > 1 && ts[i - 2].text == ".";
        if c_source && !designator_start {
            continue;
        }
        let end = paired[i].context("incomplete C/C++ array coverage: unpaired bracket")?;
        let bracket = scope.start + token.start..scope.start + token.end;
        let group = bracket.start..scope.start + ts[end].end;
        let within = |hit: &Range<usize>| {
            (group.start <= hit.start || text[hit.start..].starts_with('['))
                && hit.start < group.end
                && group.start < hit.end
                && hit.end <= group.end
        };
        if !hits.iter().any(&within) {
            continue;
        }
        let node = super::c_declarations::ast(remote, root, file, text, bracket.clone())
            .await
            .context("incomplete C/C++ array coverage; refused before writes")?;
        let queried = span(text, &group)?;
        anyhow::ensure!(
            node_span(&node).is_some_and(|(start, end)| start <= queried.0 && queried.1 <= end),
            "incomplete C/C++ array coverage: missing bracket context; refused before writes"
        );
        let array_type = node["role"] == "type"
            && matches!(
                node["kind"].as_str(),
                Some("ConstantArray" | "DependentSizedArray")
            );
        let designator = node["role"] == "expression" && node["kind"] == "DesignatedInit";
        let constant_index = span(
            text,
            &(scope.start + ts[i + 1].start..scope.start + ts[end - 1].end),
        )?;
        // The first child is the initialized value, which can be executable.
        // Only a Constant child covering this bracket's operand proves an index.
        let constant_designator = designator
            && node["children"].as_array().is_some_and(|children| {
                children.iter().skip(1).any(|child| {
                    child["role"] == "expression"
                        && child["kind"] == "Constant"
                        && node_span(child).is_some_and(|range| range == constant_index)
                })
            });
        let constant = !c_source && array_type || constant_designator;
        let runtime = node["role"] == "expression"
            && (node["kind"] == "ArraySubscript"
                || (node["kind"] == "CXXOperatorCall"
                    && node["children"]
                        .as_array()
                        .and_then(|children| children.first())
                        .is_some_and(|callee| {
                            super::c_constants::resolved_callee(callee, "operator[]")
                        })));
        let c_bound =
            c_source && (array_type || node["role"] == "type" && node["kind"] == "VariableArray");
        anyhow::ensure!(
            constant || runtime || c_bound,
            "incomplete C/C++ array coverage: unknown bracket context; refused before writes"
        );
        if constant {
            hits.retain(|hit| !within(hit));
        }
    }
    Ok(hits)
}
