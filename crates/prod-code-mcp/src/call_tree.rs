//! The call hierarchy as a tree (roadmap 7.5): who calls a function, who calls those, and so on
//! to a depth; or what it calls, transitively.
//!
//! Each level is one `callHierarchy/incomingCalls` (or `outgoingCalls`) request per function,
//! answered by the analyzer of the workspace. A function already in the tree is marked where it
//! appears again instead of being expanded a second time, so a recursive chain ends, and the
//! whole tree stops at a budget of functions and says so.

use crate::tools::execute_lsp_query;
use anyhow::Result;
use std::collections::HashSet;
use std::future::Future;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;

/// The deepest tree asked for; a larger depth is read as this.
pub const MAX_DEPTH: usize = 6;
/// Functions shown in one tree, over every level.
pub const MAX_NODES: usize = 300;

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

/// The 1-based start of a range in an LSP item.
fn start_of(value: &serde_json::Value, range: &str) -> (u64, u64) {
    let start = value.get(range).and_then(|r| r.get("start"));
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
fn key_of(item: &serde_json::Value) -> (String, u64, u64) {
    let (line, col) = start_of(item, "selectionRange");
    let uri = item.get("uri").and_then(|u| u.as_str()).unwrap_or("");
    (uri.to_string(), line, col)
}

/// The node an edge of the hierarchy stands for, without its children yet.
fn node_of(edge: &serde_json::Value, other: &serde_json::Value) -> Node {
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

struct Walk<'a> {
    remote: SocketAddr,
    root: &'a Path,
    file: &'a Path,
    incoming: bool,
    depth: usize,
    seen: HashSet<(String, u64, u64)>,
    shown: usize,
    truncated: bool,
}

impl Walk<'_> {
    /// The functions one level below `item`, each expanded while the depth and budget last.
    fn expand(
        &mut self,
        item: serde_json::Value,
        level: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Node>>> + Send + '_>> {
        Box::pin(async move {
            let method = if self.incoming {
                "callHierarchy/incomingCalls"
            } else {
                "callHierarchy/outgoingCalls"
            };
            let side = if self.incoming { "from" } else { "to" };
            let edges = execute_lsp_query(
                self.remote,
                self.root,
                self.file,
                method,
                serde_json::json!({ "item": item }),
            )
            .await?;
            let mut nodes = Vec::new();
            for edge in edges.as_array().cloned().unwrap_or_default() {
                if self.shown >= MAX_NODES {
                    self.truncated = true;
                    break;
                }
                self.shown += 1;
                let other = edge.get(side).cloned().unwrap_or_default();
                let mut node = node_of(&edge, &other);
                if !self.seen.insert(key_of(&other)) {
                    node.repeated = true;
                } else if level < self.depth {
                    node.children = self.expand(other, level + 1).await?;
                }
                nodes.push(node);
            }
            Ok(nodes)
        })
    }
}

/// The callers (`incoming`) or callees of the function at the 1-based `line`:`character` of
/// `file`, to `depth` levels (1 is the direct ones; at most [`MAX_DEPTH`]). `None` when there is
/// no function there.
pub async fn call_tree(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    incoming: bool,
    depth: usize,
) -> Result<Option<CallTree>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {file:?}"))?
        .to_string();
    let items = execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/prepareCallHierarchy",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        }),
    )
    .await?;
    let Some(item) = items.as_array().and_then(|a| a.first()).cloned() else {
        return Ok(None);
    };
    let depth = depth.clamp(1, MAX_DEPTH);
    let mut walk = Walk {
        remote,
        root,
        file,
        incoming,
        depth,
        seen: HashSet::from([key_of(&item)]),
        shown: 0,
        truncated: false,
    };
    let nodes = walk.expand(item.clone(), 1).await?;
    Ok(Some(CallTree {
        name: item
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("?")
            .to_string(),
        incoming,
        depth,
        nodes,
        truncated: walk.truncated,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(name: &str, children: Vec<Node>, repeated: bool) -> Node {
        Node {
            name: name.into(),
            uri: format!("file:///w/{name}.rs"),
            line: 1,
            col: 4,
            sites: vec!["3:5".into()],
            children,
            repeated,
        }
    }

    #[test]
    fn a_tree_is_indented_by_level_and_a_repeat_is_marked() {
        let tree = CallTree {
            name: "leaf".into(),
            incoming: true,
            depth: 3,
            nodes: vec![node(
                "mid",
                vec![node("top", vec![], false), node("leaf", vec![], true)],
                false,
            )],
            truncated: true,
        };
        assert_eq!(tree.count(), 3);
        assert_eq!(
            tree.render(),
            "`leaf`: 1 caller(s), 3 in all to depth 3\n  \
             • mid  file:///w/mid.rs:1:4  [call sites: 3:5]\n    \
             • top  file:///w/top.rs:1:4  [call sites: 3:5]\n    \
             • leaf  file:///w/leaf.rs:1:4  [call sites: 3:5]  (shown above)\n\
             … stopped at 300 functions; ask for less depth or start lower"
        );
        let empty = CallTree {
            name: "f".into(),
            incoming: false,
            depth: 1,
            nodes: vec![],
            truncated: false,
        };
        assert_eq!(empty.render(), "`f`: 0 callee(s) — no callees found.");
    }

    #[test]
    fn an_edge_gives_its_name_position_and_call_sites() {
        let other = serde_json::json!({
            "name": "caller",
            "uri": "file:///w/a.rs",
            "selectionRange": { "start": { "line": 9, "character": 3 }, "end": { "line": 9, "character": 9 } }
        });
        let edge = serde_json::json!({
            "from": other,
            "fromRanges": [
                { "start": { "line": 11, "character": 4 }, "end": { "line": 11, "character": 8 } },
                { "start": { "line": 12, "character": 0 }, "end": { "line": 12, "character": 4 } }
            ]
        });
        let node = node_of(&edge, &other);
        assert_eq!((node.name.as_str(), node.line, node.col), ("caller", 10, 4));
        assert_eq!(node.sites, vec!["12:5", "13:1"]);
        assert_eq!(key_of(&other), ("file:///w/a.rs".into(), 10, 4));
        assert_eq!(
            node_of(&serde_json::json!({}), &serde_json::json!({})).name,
            "?"
        );
    }
}
