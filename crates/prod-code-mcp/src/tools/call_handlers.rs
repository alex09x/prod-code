/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use url::Url;

use super::{execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_implementations(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/implementation",
        params,
    )
    .await?;
    let arr = match &res {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![res.clone()],
        _ => Vec::new(),
    };
    if arr.is_empty() {
        return Ok(McpToolCallResult::text(
            "No implementations found.".to_string(),
        ));
    }
    let mut out = format!("Found {} implementation(s):\n", arr.len());
    for loc in &arr {
        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = loc.get("range").and_then(|r| r.get("start"));
        let l = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let c = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        let mut snippet = String::new();
        let target_path = url::Url::parse(uri)
            .ok()
            .and_then(|u| u.to_file_path().ok())
            .unwrap_or_else(|| PathBuf::from(crate::remote_fs::uri_to_path(uri)));
        let content_opt = std::fs::read_to_string(&target_path).ok();
        if let Some(content) = content_opt {
            if let Some(line_str) = content.lines().nth(l.saturating_sub(1) as usize) {
                let trimmed = line_str.trim();
                if !trimmed.is_empty() {
                    snippet = format!("  `{trimmed}`");
                }
            }
        } else if let Ok((bytes, _)) =
            crate::remote_fs::read_source(remote, workspace_root, &target_path.to_string_lossy())
                .await
        {
            let content = String::from_utf8_lossy(&bytes);
            if let Some(line_str) = content.lines().nth(l.saturating_sub(1) as usize) {
                let trimmed = line_str.trim();
                if !trimmed.is_empty() {
                    snippet = format!("  `{trimmed}`");
                }
            }
        }
        out.push_str(&format!("  • {uri}:{l}:{c}{snippet}\n"));
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}

pub(crate) async fn handle_callers(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let incoming = tool_name == "code_callers";
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let file_path = resolve_file_path(workspace_root, path_str);
    let tree = || {
        crate::call_tree::call_tree(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            incoming,
            depth,
        )
    };
    let mut found = tree().await?;
    let mut note = None;
    if incoming
        && found.as_ref().is_some_and(|t| t.nodes.is_empty())
        && let Some((built, text)) = build_swift_index(remote, workspace_root, &file_path).await
    {
        note = Some(text);
        if built {
            found = tree().await?;
        }
    }
    let body = match found {
        Some(tree) => tree.render(),
        None => format!("No function at {path_str}:{line}:{character}."),
    };
    Ok(McpToolCallResult::text(match note {
        Some(note) => format!("{note}\n{body}"),
        None => body,
    }))
}

/// The SwiftPM packages whose index this process built, so a search asks for a build once.
fn swift_indexes_built() -> &'static std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>
{
    static BUILT: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    > = std::sync::OnceLock::new();
    BUILT.get_or_init(Default::default)
}

/// The SwiftPM package a Swift file belongs to: the nearest directory above it, inside `root`,
/// with a `Package.swift`.
fn swift_package_of(root: &Path, file: &Path) -> Option<std::path::PathBuf> {
    if file.extension().and_then(|e| e.to_str()) != Some("swift") {
        return None;
    }
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let file = std::fs::canonicalize(file).ok()?;
    file.ancestors()
        .skip(1)
        .take_while(|dir| dir.starts_with(&root))
        .find(|dir| dir.join("Package.swift").is_file())
        .map(Path::to_path_buf)
}

/// Builds the index of the SwiftPM package `file` is in, on that package's node, once per
/// process, for a search in it that found nothing. sourcekit-lsp finds a use in another file
/// only through the index store a build leaves (#166); in a package never built on the node it
/// answers with nothing, which reads like "nothing uses this" (#358). The running server picks
/// the new store up. Returns whether the build succeeded and a line saying what was done;
/// `None` for a file in no package, or in one this process built already.
pub(crate) async fn build_swift_index(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
) -> Option<(bool, String)> {
    let package = swift_package_of(root, file)?;
    if !swift_indexes_built()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(package.clone())
    {
        return None;
    }
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let subdir = package
        .strip_prefix(&canonical_root)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|p| !p.is_empty());
    let package_named = subdir.as_deref().map_or_else(
        || "the package".to_string(),
        |dir| format!("the package in {dir}"),
    );
    let node = crate::cluster::route_for_path(remote, root, package.to_str())
        .await
        .unwrap_or(remote);
    let mut output = Vec::new();
    let outcome = crate::exec::run_remote(
        node,
        root,
        subdir.as_deref(),
        vec![
            "swift".to_string(),
            "build".to_string(),
            "--build-tests".to_string(),
        ],
        Vec::new(),
        900,
        false,
        |_, bytes| {
            output.extend_from_slice(bytes);
            let excess = output.len().saturating_sub(4096);
            output.drain(..excess);
        },
    )
    .await;
    let why = |outcome: String| {
        format!(
            "sourcekit-lsp finds uses in other files only through a build's index, and \
             `swift build --build-tests` for {package_named} {outcome}: uses outside this file \
             may be missing."
        )
    };
    // What the build said went wrong: its last line naming an error, else its last line.
    let text = String::from_utf8_lossy(&output);
    let last_line = text
        .lines()
        .rev()
        .find(|l| l.to_ascii_lowercase().contains("error"))
        .or_else(|| text.lines().rev().find(|l| !l.trim().is_empty()))
        .map(|l| l.trim().chars().take(200).collect::<String>())
        .unwrap_or_default();
    Some(match outcome {
        Ok(o) if o.exit.exit_code == Some(0) && !o.exit.timed_out => (
            true,
            format!(
                "sourcekit-lsp finds uses in other files through a build's index: built \
                 {package_named} first (`swift build --build-tests`, {:.1} s).",
                o.exit.duration_ms as f64 / 1000.0
            ),
        ),
        Ok(o) if o.exit.timed_out => (false, why("timed out".to_string())),
        Ok(o) => (
            false,
            why(format!(
                "failed (exit {}: {last_line})",
                o.exit.exit_code.map_or("?".to_string(), |c| c.to_string())
            )),
        ),
        Err(err) => (false, why(format!("could not run ({err:#})"))),
    })
}
