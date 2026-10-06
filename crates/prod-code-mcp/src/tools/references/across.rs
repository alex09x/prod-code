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

use anyhow::{Context, Result};

use crate::protocol::McpToolCallResult;
use crate::tools::{execute_tool, resolve_file_path};

/// `code_references` in this checkout and in each directory of `also_in`, the name resolved in
/// each, every answer under its checkout's directory; a checkout that fails says why without
/// hiding the others (#375).
pub(crate) async fn references_across(
    remote: SocketAddr,
    workspace_root: &Path,
    args: serde_json::Value,
    dirs: &[serde_json::Value],
) -> Result<McpToolCallResult> {
    if args
        .get("symbol")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.trim().is_empty())
    {
        anyhow::bail!(
            "`also_in` goes with `symbol`: a position is a place in one checkout, a name is \
             resolved in each"
        );
    }
    let mut roots = vec![workspace_root.to_path_buf()];
    for dir in dirs {
        let dir = dir
            .as_str()
            .context("`also_in` lists the directories of other checkouts")?;
        let dir = resolve_file_path(workspace_root, dir);
        let dir = std::fs::canonicalize(&dir).unwrap_or(dir);
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }
    let mut out = String::new();
    let mut found = 0usize;
    for (index, root) in roots.iter().enumerate() {
        let mut asked = args.clone();
        if let Some(obj) = asked.as_object_mut() {
            obj.remove("also_in");
            // A file hint names a file of the first checkout.
            if index > 0 {
                obj.remove("path");
            }
        }
        // Each checkout is asked on the node its own workspace is placed on.
        let node = if index == 0 || !root.is_dir() {
            Ok(remote)
        } else {
            crate::cluster::route_for_checkout(remote, root).await
        };
        let text = match node {
            _ if !root.is_dir() => "not a directory".to_string(),
            Err(err) => format!("{err:#}"),
            Ok(node) => match Box::pin(execute_tool(node, root, "code_references", asked)).await {
                Ok(result) => result
                    .content
                    .iter()
                    .map(|item| {
                        let crate::protocol::McpContentItem::Text { text } = item;
                        text.as_str()
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(err) => format!("{err:#}"),
            },
        };
        found += text.lines().filter(|l| l.starts_with("  • ")).count();
        out.push_str(&format!(
            "== {} ==\n{}\n\n",
            root.display(),
            text.trim_end()
        ));
    }
    out.push_str(&format!(
        "{found} reference(s) in {} checkout(s)",
        roots.len()
    ));
    Ok(McpToolCallResult::text(out))
}

/// The text of 1-based `line` of `file`: read here, or from the node for a file only the node
/// has (a dependency's source). `None` when it cannot be read.
pub(crate) async fn position_line(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
) -> Option<String> {
    let text = match std::fs::read_to_string(file) {
        Ok(text) => text,
        Err(_) if crate::remote_fs::is_external(root, &file.to_string_lossy()) => {
            let (bytes, _) = crate::remote_fs::read_remote_file(remote, &file.to_string_lossy(), 0)
                .await
                .ok()?;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        Err(_) => return None,
    };
    text.lines()
        .nth((line as usize).checked_sub(1)?)
        .map(str::to_string)
}

/// The name a 1-based `character` of a line stands on, or just after (where an editor's cursor
/// sits at the end of a word). `None` for a position on no name.
pub(crate) fn name_at(line: &str, character: u32) -> Option<String> {
    let chars: Vec<char> = line.chars().collect();
    let is_name = |c: &char| c.is_alphanumeric() || *c == '_';
    let at = (character as usize).checked_sub(1)?;
    let at = if chars.get(at).is_some_and(is_name) {
        at
    } else if at > 0 && chars.get(at - 1).is_some_and(is_name) {
        at - 1
    } else {
        return None;
    };
    let start = chars[..at]
        .iter()
        .rposition(|c| !is_name(c))
        .map_or(0, |i| i + 1);
    let end = chars[at..]
        .iter()
        .position(|c| !is_name(c))
        .map_or(chars.len(), |i| at + i);
    Some(chars[start..end].iter().collect())
}
