/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;

use super::cache::MAX_NODES;

/// One function in the tree, with where it calls (or is called) and what is below it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub name: String,
    pub uri: String,
    /// 1-based position of its name.
    pub line: u64,
    pub col: u64,
    /// 1-based `line:col` of each call between it and its parent.
    pub sites: Vec<String>,
    pub children: Vec<Node>,
    /// Already shown higher in the tree, so not expanded here.
    pub repeated: bool,
}

/// The tree below one function.
#[derive(Debug, Clone, PartialEq)]
pub struct CallTree {
    pub name: String,
    pub incoming: bool,
    pub depth: usize,
    pub nodes: Vec<Node>,
    /// The budget ran out before the tree was complete.
    pub truncated: bool,
}

impl CallTree {
    /// Every function in the tree, at every level.
    pub fn count(&self) -> usize {
        fn count(nodes: &[Node]) -> usize {
            nodes.iter().map(|n| 1 + count(&n.children)).sum()
        }
        count(&self.nodes)
    }

    pub fn render(&self) -> String {
        let kind = if self.incoming { "caller" } else { "callee" };
        let mut out = format!("`{}`: {} {kind}(s)", self.name, self.nodes.len());
        if self.nodes.is_empty() {
            out.push_str(&format!(" — no {kind}s found."));
            return out;
        }
        if self.depth > 1 {
            out.push_str(&format!(
                ", {} in all to depth {}",
                self.count(),
                self.depth
            ));
        }
        out.push('\n');
        fn walk(out: &mut String, nodes: &[Node], indent: usize) {
            for node in nodes {
                out.push_str(&format!(
                    "{}• {}  {}:{}:{}  [call sites: {}]{}\n",
                    "  ".repeat(indent),
                    node.name,
                    node.uri,
                    node.line,
                    node.col,
                    node.sites.join(", "),
                    if node.repeated { "  (shown above)" } else { "" }
                ));
                walk(out, &node.children, indent + 1);
            }
        }
        walk(&mut out, &self.nodes, 1);
        if self.truncated {
            out.push_str(&format!(
                "… stopped at {MAX_NODES} functions; ask for less depth or start lower\n"
            ));
        }
        out.trim_end().to_string()
    }
}

/// The range to use for an LSP hierarchy item. Keep the legacy range fallback only when
/// selectionRange is absent; a present but malformed selection range must be rejected.
pub(crate) fn hierarchy_range<'a>(
    value: &'a serde_json::Value,
    range: &str,
) -> Option<&'a serde_json::Value> {
    match value.get(range) {
        Some(range) => Some(range),
        None if range == "selectionRange" => value.get("range"),
        None => None,
    }
}

/// The 1-based start of a range in an LSP item.
pub(crate) fn start_of(value: &serde_json::Value, range: &str) -> (u64, u64) {
    let start = hierarchy_range(value, range).and_then(|r| r.get("start"));
    let at = |key: &str| {
        start
            .and_then(|s| s.get(key))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            + 1
    };
    (at("line"), at("character"))
}

/// A call hierarchy item's identity: its file and where its name is.
pub(crate) fn key_of(item: &serde_json::Value) -> (String, u64, u64) {
    let (line, col) = start_of(item, "selectionRange");
    let uri = item.get("uri").and_then(|u| u.as_str()).unwrap_or("");
    (uri.to_string(), line, col)
}

pub(crate) fn lsp_position(position: Option<&serde_json::Value>) -> Option<(u64, u64)> {
    let line = position?.get("line")?.as_u64()?;
    let character = position?.get("character")?.as_u64()?;
    (line < u64::from(u32::MAX) && character < u64::from(u32::MAX)).then_some((line, character))
}

pub(crate) fn valid_lsp_range(range: &serde_json::Value) -> bool {
    matches!(
        (lsp_position(range.get("start")), lsp_position(range.get("end"))),
        (Some(start), Some(end)) if start <= end
    )
}

pub(crate) fn validate_call_hierarchy_item(item: &serde_json::Value) -> Result<()> {
    anyhow::ensure!(
        item.is_object(),
        "call hierarchy item is not an object: {item}"
    );
    anyhow::ensure!(
        item.get("name").and_then(|n| n.as_str()).is_some(),
        "call hierarchy item has no valid 'name': {item}"
    );
    anyhow::ensure!(
        item.get("uri").and_then(|u| u.as_str()).is_some(),
        "call hierarchy item has no valid 'uri': {item}"
    );
    let has_valid_range = hierarchy_range(item, "selectionRange").is_some_and(valid_lsp_range);
    anyhow::ensure!(
        has_valid_range,
        "call hierarchy item has no valid 'selectionRange' or 'range': {item}"
    );
    Ok(())
}

/// The node an edge of the hierarchy stands for, without its children yet.
pub(crate) fn node_of(edge: &serde_json::Value, other: &serde_json::Value) -> Node {
    let (uri, line, col) = key_of(other);
    let sites = edge
        .get("fromRanges")
        .and_then(|r| r.as_array())
        .map(|ranges| {
            ranges
                .iter()
                .map(|r| {
                    let (l, c) = start_of(&serde_json::json!({ "range": r }), "range");
                    format!("{l}:{c}")
                })
                .collect()
        })
        .unwrap_or_default();
    Node {
        name: other
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        uri,
        line,
        col,
        sites,
        children: Vec::new(),
        repeated: false,
    }
}
