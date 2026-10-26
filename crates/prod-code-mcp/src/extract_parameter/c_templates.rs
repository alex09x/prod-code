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
use serde_json::Value;
use std::{net::SocketAddr, ops::Range, path::Path};

type Position = (u64, u64);
type Span = (Position, Position);

pub(super) struct Cast {
    pub range: Range<usize>,
    pub operand: Range<usize>,
}

/// Parse casts as bounded primaries using the analyzer's actual type/operand
/// boundaries. In C headers the same spelling may instead be an identifier.
/// This discovery pass is opportunistic: preserve() requires complete coverage
/// for actual replacements. An unrelated unsupported or inactive cast must not
/// make the entire declaration unavailable.
pub(super) async fn casts(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    syntax: Syntax,
    scope: Range<usize>,
) -> Result<Vec<Cast>> {
    if !syntax.is_c_family()
        || (syntax == Syntax::C && file.extension().is_some_and(|ext| ext == "c"))
    {
        return Ok(Vec::new());
    }
    let ts = tokens(&text[scope.clone()], syntax)?;
    let mut casts = Vec::new();
    for pair in ts.windows(2) {
        if pair[0].text != "static_cast" || pair[1].text != "<" {
            continue;
        }
        let angle = scope.start + pair[1].start..scope.start + pair[1].end;
        let Ok(node) = super::c_declarations::ast(remote, root, file, text, angle).await else {
            continue;
        };
        if let Ok(Some(cast)) =
            bounded_cast(&node, text, &ts, scope.clone(), scope.start + pair[0].start)
        {
            casts.push(cast);
        }
    }
    Ok(casts)
}

/// A conditional branch is executable only outside required constant operands.
/// Ask clangd at the angle token, where both type and function templates expose
/// explicit template-argument ranges. A literal-only query loses that ancestry.
/// The same query distinguishes runtime comparisons without a type-name list.
pub(super) async fn preserve(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    text: &str,
    syntax: Syntax,
    scope: Range<usize>,
    mut hits: Vec<Range<usize>>,
) -> Result<Vec<Range<usize>>> {
    if syntax == Syntax::C && file.extension().is_some_and(|ext| ext == "c") {
        return Ok(hits);
    }
    let ts = tokens(&text[scope.clone()], syntax)?;
    let paired = pairs(&ts)?;
    for (i, token) in ts.iter().enumerate().filter(|(_, t)| t.text == "<") {
        let end = seek(&ts, &paired, i + 1, &[";", "}"]);
        let end = ts.get(end).map_or(scope.end, |t| scope.start + t.start);
        let angle = scope.start + token.start..scope.start + token.end;
        // Ignore angle tokens in signatures, unrelated statements and already
        // excluded constants. No analyzer work is needed for ordinary literals.
        if !hits
            .iter()
            .any(|hit| angle.start < hit.end && hit.start < end)
        {
            continue;
        }
        let node = super::c_declarations::ast(remote, root, file, text, angle.clone()).await?;
        let queried = span(text, &angle)?;
        let context = node_span(&node);
        anyhow::ensure!(
            context.is_some_and(|(start, end)| start <= queried.0 && queried.1 <= end),
            "incomplete C/C++ template coverage: missing angle context; refused before writes"
        );
        let context = context.unwrap();
        if i > 0
            && ts[i - 1].text == "static_cast"
            && (syntax == Syntax::Cpp || node["kind"] == "CXXStaticCast")
        {
            // clangd represents class-target conversions as CXXConstruct. Keep
            // that region opaque, without requiring its operand shape for a
            // separate literal even in the same statement.
            let mut overlaps = false;
            for hit in &hits {
                let (start, end) = span(text, hit)?;
                overlaps |= start < context.1 && context.0 < end;
            }
            if !overlaps {
                continue;
            }
            anyhow::ensure!(
                bounded_cast(
                    &node,
                    text,
                    &ts,
                    scope.clone(),
                    scope.start + ts[i - 1].start
                )?
                .is_some(),
                "incomplete C/C++ cast coverage: missing cast context; refused before writes"
            );
        }
        let mut arguments = Vec::new();
        template_arguments(&node, &mut arguments)?;
        let cast = cast_operand(&node)?;
        // An entire template type is outside executable grammar, including an
        // empty specialization. Resolved empty function/member template calls
        // have no argument children; only their surrounding values may change.
        let template_type = node["role"] == "type" && node["kind"] == "TemplateSpecialization";
        if template_type {
            arguments.push(context);
        }
        let resolved_value = node["role"] == "expression"
            && matches!(node["kind"].as_str(), Some("DeclRef" | "Member"));
        let mut operand_hit = false;
        for hit in &hits {
            let (start, end) = span(text, hit)?;
            operand_hit |= context.0 <= start && end <= context.1;
        }
        let comparison = node["role"] == "expression"
            && matches!(
                node["kind"].as_str(),
                Some("BinaryOperator" | "CXXOperatorCall")
            )
            && node["detail"] == "<";
        anyhow::ensure!(
            !arguments.is_empty()
                || cast.is_some()
                || comparison
                || (resolved_value && !operand_hit),
            "incomplete C/C++ template coverage: unknown angle context; refused before writes"
        );
        let mut retained = Vec::new();
        for hit in hits {
            let (start, end) = span(text, &hit)?;
            let valid_cast = cast.is_none_or(|(cast, operand)| {
                end <= cast.0
                    || cast.1 <= start
                    || (operand.0 <= start && end <= operand.1)
                    || (start <= cast.0 && cast.1 <= end)
            });
            // A complete runtime call such as f<80>() may still be extracted;
            // only operands contained in its template arguments must stay put.
            if valid_cast && !arguments.iter().any(|(a, b)| *a <= start && end <= *b) {
                retained.push(hit);
            }
        }
        hits = retained;
    }
    Ok(hits)
}

fn bounded_cast(
    node: &Value,
    text: &str,
    ts: &[Token<'_>],
    scope: Range<usize>,
    start: usize,
) -> Result<Option<Cast>> {
    let Some((context, operand)) = cast_operand(node)? else {
        return Ok(None);
    };
    let bytes = |(start, end): Span| -> Result<Range<usize>> {
        let offset = |(line, col): Position| {
            crate::signature::offset_of(
                text,
                u32::try_from(line).ok()?.checked_add(1)?,
                u32::try_from(col).ok()?.checked_add(1)?,
            )
        };
        Ok(offset(start).context("incomplete C/C++ cast start")?
            ..offset(end).context("incomplete C/C++ cast end")?)
    };
    let cast = Cast {
        range: bytes(context)?,
        operand: bytes(operand)?,
    };
    let starts = |at| ts.iter().any(|t| scope.start + t.start == at);
    let ends = |at| ts.iter().any(|t| scope.start + t.end == at);
    anyhow::ensure!(
        cast.range.start == start
            && cast.range.end <= scope.end
            && ends(cast.range.end)
            && starts(cast.operand.start)
            && ends(cast.operand.end),
        "incomplete C/C++ cast coverage: invalid token boundaries; refused before writes"
    );
    Ok(Some(cast))
}

fn cast_operand(node: &Value) -> Result<Option<(Span, Span)>> {
    if node["role"] != "expression" || node["kind"] != "CXXStaticCast" {
        return Ok(None);
    }
    let children = node["children"]
        .as_array()
        .context("incomplete C/C++ cast coverage: missing children; refused before writes")?;
    anyhow::ensure!(
        children.len() == 2 && children[0]["role"] == "type" && children[1]["role"] == "expression",
        "incomplete C/C++ cast coverage: unknown type/operand context; refused before writes"
    );
    let context = node_span(node).context("incomplete C/C++ cast range")?;
    let ty = node_span(&children[0]).context("incomplete C/C++ cast type range")?;
    let operand = node_span(&children[1]).context("incomplete C/C++ cast operand range")?;
    anyhow::ensure!(
        context.0 < ty.0 && ty.1 < operand.0 && operand.1 < context.1,
        "incomplete C/C++ cast coverage: invalid type/operand ranges; refused before writes"
    );
    // Candidates strictly inside the cast must stay within its actual operand
    // child; the target type and required cast parentheses are not expressions.
    Ok(Some((context, operand)))
}

fn template_arguments(node: &Value, arguments: &mut Vec<Span>) -> Result<()> {
    if node["role"] == "template argument" {
        arguments.push(node_span(node).context(
            "incomplete C/C++ template coverage: missing argument range; refused before writes",
        )?);
        return Ok(());
    }
    if let Some(children) = node["children"].as_array() {
        for child in children {
            template_arguments(child, arguments)?;
        }
    }
    Ok(())
}

pub(super) fn node_span(node: &Value) -> Option<Span> {
    let position = |key: &str| {
        let value = &node["range"][key];
        Some((value["line"].as_u64()?, value["character"].as_u64()?))
    };
    let (start, end) = (position("start")?, position("end")?);
    (start < end).then_some((start, end))
}

pub(super) fn span(text: &str, range: &Range<usize>) -> Result<Span> {
    let position = |at| -> Result<Position> {
        let (line, col) = crate::signature::position_at(text, at)?;
        Ok((u64::from(line - 1), u64::from(col - 1)))
    };
    Ok((position(range.start)?, position(range.end)?))
}
