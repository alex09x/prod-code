/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};

use super::lexer::bare;
use super::types::OutlineNode;

/// The smallest struct-like node called `name` whose lines hold `line` (1-based); `None` when
/// the outline has none. An outline that cannot be read where it speaks of `name` (a range that
/// is not one, a field without a name, members that are not a list) is an error: dropping that
/// part would pass a partial or misplaced struct for the declaration.
pub(crate) fn outline_node(
    symbols: &serde_json::Value,
    name: &str,
    line: u32,
) -> Result<Option<OutlineNode>> {
    fn walk(
        nodes: &[serde_json::Value],
        name: &str,
        line: u32,
        best: &mut Option<OutlineNode>,
    ) -> Result<()> {
        for node in nodes {
            let named = node
                .get("name")
                .and_then(|n| n.as_str())
                .is_some_and(|n| bare(n) == bare(name));
            // Struct, enum and class: a union is listed as a struct.
            let kind = node.get("kind").and_then(|k| k.as_u64());
            if named && matches!(kind, Some(5) | Some(10) | Some(23)) {
                let (s, e) = symbol_lines(node).with_context(|| {
                    format!("it gives `{name}` a range that is not one: {}", shown(node))
                })?;
                if s <= line && line <= e && best.as_ref().is_none_or(|b| e - s < b.end - b.start) {
                    let mut fields = Vec::new();
                    for child in children(node)? {
                        match child.get("kind").and_then(|k| k.as_u64()) {
                            Some(8) => match child.get("name").and_then(|n| n.as_str()) {
                                Some(field) => fields.push(field.to_string()),
                                None => {
                                    bail!("it lists a field of `{name}` without a name: {child}")
                                }
                            },
                            Some(_) => {}
                            None => bail!("it lists a member of `{name}` without a kind: {child}"),
                        }
                    }
                    *best = Some(OutlineNode {
                        start: s,
                        end: e,
                        fields: (!fields.is_empty()).then_some(fields),
                    });
                }
            }
            walk(children(node)?, name, line, best)?;
        }
        Ok(())
    }
    /// Fields listed flat, as the Rust engine answers: a `Field` whose container path ends in
    /// the struct's name and whose line is inside the struct's.
    fn flat_fields(
        nodes: &[serde_json::Value],
        owner: &str,
        lines: (u32, u32),
        out: &mut Vec<String>,
    ) -> Result<()> {
        for node in nodes {
            let contained = node
                .get("containerName")
                .and_then(|c| c.as_str())
                .and_then(|c| c.rsplit(" > ").next())
                .is_some_and(|c| bare(c.trim()) == bare(owner));
            if node.get("kind").and_then(|k| k.as_u64()) == Some(8) && contained {
                let field = node.get("name").and_then(|n| n.as_str()).with_context(|| {
                    format!("it lists a field of `{owner}` without a name: {node}")
                })?;
                let (start, _) = symbol_lines(node).with_context(|| {
                    format!(
                        "it gives field `{field}` of `{owner}` a range that is not one: {}",
                        shown(node)
                    )
                })?;
                if lines.0 <= start && start <= lines.1 {
                    out.push(field.to_string());
                }
            }
            flat_fields(children(node)?, owner, lines, out)?;
        }
        Ok(())
    }
    /// A node's `DocumentSymbol` or `SymbolInformation` range, as 1-based lines.
    fn symbol_lines(node: &serde_json::Value) -> Option<(u32, u32)> {
        node.get("range")
            .or_else(|| node.pointer("/location/range"))
            .and_then(range_lines)
    }
    /// The range a node was given, for an error.
    fn shown(node: &serde_json::Value) -> String {
        node.get("range")
            .or_else(|| node.pointer("/location/range"))
            .map_or_else(|| "none".to_string(), |r| r.to_string())
    }
    /// A node's members: none when it has no `children` (or `null`), an error when they are
    /// not a list.
    fn children(node: &serde_json::Value) -> Result<&[serde_json::Value]> {
        match node.get("children") {
            None | Some(serde_json::Value::Null) => Ok(&[][..]),
            Some(serde_json::Value::Array(children)) => Ok(children.as_slice()),
            Some(other) => bail!("it lists members that are not a list: {other}"),
        }
    }
    let nodes = match symbols {
        serde_json::Value::Null => return Ok(None),
        serde_json::Value::Array(nodes) => nodes,
        other => bail!("it is not a list of symbols: {other}"),
    };
    let mut best: Option<OutlineNode> = None;
    walk(nodes, name, line, &mut best)?;
    let Some(mut best) = best else {
        return Ok(None);
    };
    if best.fields.is_none() {
        let mut fields = Vec::new();
        flat_fields(nodes, name, (best.start, best.end), &mut fields)?;
        best.fields = (!fields.is_empty()).then_some(fields);
    }
    Ok(Some(best))
}

/// A 0-based LSP line or character as a 1-based `u32`. `None` for anything else, including a
/// number a cast would truncate or `+ 1` would overflow.
pub(crate) fn one_based(value: Option<&serde_json::Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?).ok()?.checked_add(1)
}

/// The first and last lines of an LSP range, 1-based. `None` unless both ends have a line and a
/// character that fit and the range does not end on a line before it starts. Only the lines are
/// ordered: the Rust engine's flat outline ends a field at character 0 of its own line.
pub(crate) fn range_lines(range: &serde_json::Value) -> Option<(u32, u32)> {
    let line = |at: &str| {
        one_based(range.pointer(&format!("/{at}/character")))?;
        one_based(range.pointer(&format!("/{at}/line")))
    };
    let (start, end) = (line("start")?, line("end")?);
    (start <= end).then_some((start, end))
}
