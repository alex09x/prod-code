/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Definition retrieval, body extraction, doc comment recovery, and line numbering.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use url::Url;

use super::{execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_definition(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("file"))
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
    let body = args.get("body").and_then(|v| v.as_bool()).unwrap_or(true);
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/definition",
        params,
    )
    .await?;
    let mut out = String::new();
    let has_locations = match &res {
        serde_json::Value::Array(arr) => !arr.is_empty(),
        serde_json::Value::Object(_) => true,
        _ => false,
    };
    if has_locations {
        if let Some(arr) = res.as_array() {
            for (i, loc) in arr.iter().enumerate() {
                let uri = loc
                    .get("uri")
                    .or_else(|| loc.get("targetUri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                let start_line = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
                if body && i < 3 {
                    match definition_body(
                        remote,
                        workspace_root,
                        uri,
                        start_line as u32,
                        start_col as u32,
                    )
                    .await
                    {
                        Ok(text) => {
                            out.push('\n');
                            out.push_str(&text);
                        }
                        Err(e) => out.push_str(&format!(
                            "\n   (the definition's code could not be read: {e:#})"
                        )),
                    }
                    continue;
                }
                // Outside the checkout the file exists only on the gateway: include
                // the lines around the definition so the agent can read it.
                let path = crate::remote_fs::uri_to_path(uri);
                if i < 3 && crate::remote_fs::is_external(workspace_root, &path) {
                    match crate::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            out.push('\n');
                            out.push_str(&crate::remote_fs::snippet(&text, start_line as u32, 8));
                        }
                        Err(e) => {
                            out.push_str(&format!("\n   (external source not readable: {e})"))
                        }
                    }
                }
            }
        } else if let Some(obj) = res.as_object() {
            let uri = obj
                .get("uri")
                .or_else(|| obj.get("targetUri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let range = obj.get("range").or_else(|| obj.get("targetSelectionRange"));
            let start_line = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            let start_col = range
                .and_then(|r| r.get("start"))
                .and_then(|s| s.get("character"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                + 1;
            out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
            if body {
                match definition_body(
                    remote,
                    workspace_root,
                    uri,
                    start_line as u32,
                    start_col as u32,
                )
                .await
                {
                    Ok(text) => {
                        out.push('\n');
                        out.push_str(&text);
                    }
                    Err(e) => out.push_str(&format!(
                        "\n   (the definition's code could not be read: {e:#})"
                    )),
                }
            } else {
                let path = crate::remote_fs::uri_to_path(uri);
                if crate::remote_fs::is_external(workspace_root, &path) {
                    match crate::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            out.push('\n');
                            out.push_str(&crate::remote_fs::snippet(&text, start_line as u32, 8));
                        }
                        Err(e) => {
                            out.push_str(&format!("\n   (external source not readable: {e})"))
                        }
                    }
                }
            }
        }
    } else {
        let is_def = if args.get("symbol").is_some() {
            true
        } else {
            is_outline_definition(remote, workspace_root, &file_path, line, character).await
        };
        if is_def {
            out.push_str(&format!("📍 Definition: {file_uri}:{line}:{character}"));
            if body {
                match definition_body(remote, workspace_root, &file_uri, line, character).await {
                    Ok(text) => {
                        out.push('\n');
                        out.push_str(&text);
                    }
                    Err(e) => out.push_str(&format!(
                        "\n   (the definition's code could not be read: {e:#})"
                    )),
                }
            }
        } else {
            out.push_str("No definition found.");
        }
    }
    Ok(McpToolCallResult::text(out))
}

pub(crate) async fn is_outline_definition(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    line: u32,
    character: u32,
) -> bool {
    let Ok(uri) = Url::from_file_path(path) else {
        return false;
    };
    let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
    let Ok(outline) =
        execute_lsp_query(remote, root, path, "textDocument/documentSymbol", params).await
    else {
        return false;
    };
    let zero_line = (line as usize).saturating_sub(1);
    let zero_col = (character as usize).saturating_sub(1);
    outline_contains_definition(&outline, zero_line, zero_col)
}

pub(crate) fn outline_contains_definition(
    symbols: &serde_json::Value,
    line: usize,
    col: usize,
) -> bool {
    for sym in symbols.as_array().into_iter().flatten() {
        // Prefer selectionRange (the symbol's exact identifier range) so that positions
        // on `fn`, parameters or other tokens on the declaration line do not falsely match.
        let sel = sym
            .get("selectionRange")
            .or_else(|| sym.pointer("/location/range"))
            .or_else(|| sym.get("range"));
        if let Some(range) = sel
            && range_contains(range, line, col)
        {
            return true;
        }
        if let Some(children) = sym.get("children")
            && outline_contains_definition(children, line, col)
        {
            return true;
        }
    }
    false
}

pub(crate) fn range_contains(range: &serde_json::Value, line: usize, col: usize) -> bool {
    let Some(start) = range.get("start") else {
        return false;
    };
    let Some(start_line) = start
        .get("line")
        .and_then(|l| l.as_u64())
        .map(|l| l as usize)
    else {
        return false;
    };
    let start_col = start
        .get("character")
        .and_then(|c| c.as_u64())
        .map(|c| c as usize)
        .unwrap_or(0);
    let (end_line, end_col) = match range.get("end") {
        Some(end) => {
            let el = end
                .get("line")
                .and_then(|l| l.as_u64())
                .map(|l| l as usize)
                .unwrap_or(start_line);
            let ec = end
                .get("character")
                .and_then(|c| c.as_u64())
                .map(|c| c as usize)
                .unwrap_or(usize::MAX);
            (el, ec)
        }
        None => (start_line, usize::MAX),
    };

    if line < start_line || line > end_line {
        return false;
    }
    if line == start_line && col < start_col {
        return false;
    }
    if line == end_line && col >= end_col {
        return false;
    }
    true
}

/// The most lines of a definition `body: true` shows.
pub(crate) const MAX_BODY_LINES: usize = 300;

/// The code of the definition at 1-based `line`/`col` of `uri`, numbered (#306): the item's
/// range from the file's outline or, for a file outside the checkout or a server without an
/// outline, the lines its brackets (or, after a `:`, its indentation) span. The doc comments,
/// attributes and decorators right above it come with it.
pub(crate) async fn definition_body(
    remote: SocketAddr,
    root: &Path,
    uri: &str,
    line: u32,
    col: u32,
) -> Result<String> {
    let path = crate::remote_fs::uri_to_path(uri);
    let external = crate::remote_fs::is_external(root, &path);
    let text = if external {
        let (bytes, _) = crate::remote_fs::read_remote_file(remote, &path, 0).await?;
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?
    };
    let lines: Vec<&str> = text.lines().collect();
    anyhow::ensure!(!lines.is_empty(), "{path} is empty");
    let start = (line as usize).saturating_sub(1).min(lines.len() - 1);
    let outlined = if external {
        None
    } else {
        outlined_range(
            remote,
            root,
            Path::new(&path),
            start,
            col.saturating_sub(1) as usize,
        )
        .await
    };
    let (first, last) = match outlined {
        Some((first, last)) if last >= start => (first.min(start), last),
        _ => (start, item_end(&lines, start)),
    };
    Ok(numbered_lines(
        &lines,
        with_leading_docs(&lines, first),
        last,
    ))
}

/// The 0-based first and last lines of the innermost symbol of `path`'s outline whose range
/// holds the 0-based position.
pub(crate) async fn outlined_range(
    remote: SocketAddr,
    root: &Path,
    path: &Path,
    line: usize,
    col: usize,
) -> Option<(usize, usize)> {
    let uri = Url::from_file_path(path).ok()?;
    let params = serde_json::json!({ "textDocument": { "uri": uri.to_string() } });
    let outline = execute_lsp_query(remote, root, path, "textDocument/documentSymbol", params)
        .await
        .ok()?;
    let mut best: Option<Span> = None;
    innermost_holding(&outline, (line, col), &mut best);
    best.map(|(start, end)| (start.0, end.0))
}

/// An outline range: its 0-based (line, character) start and end.
type Span = ((usize, usize), (usize, usize));

/// Walks an outline, nested or flat, for the smallest range that holds `at`.
pub(crate) fn innermost_holding(
    symbols: &serde_json::Value,
    at: (usize, usize),
    best: &mut Option<Span>,
) {
    let point = |p: Option<&serde_json::Value>| -> Option<(usize, usize)> {
        let p = p?;
        Some((
            p.get("line")?.as_u64()? as usize,
            p.get("character")?.as_u64()? as usize,
        ))
    };
    for symbol in symbols.as_array().into_iter().flatten() {
        let range = symbol
            .get("range")
            .or_else(|| symbol.pointer("/location/range"));
        if let (Some(start), Some(end)) = (
            point(range.and_then(|r| r.get("start"))),
            point(range.and_then(|r| r.get("end"))),
        ) && start <= at
            && at <= end
            && best.is_none_or(|(s, e)| (end.0 - start.0, end.1) < (e.0 - s.0, e.1))
        {
            *best = Some((start, end));
        }
        if let Some(children) = symbol.get("children") {
            innermost_holding(children, at, best);
        }
    }
}

/// The last line of the item that starts on 0-based line `start`: where its first `{` is
/// closed; for a line that ends in `:` (Python), the last line indented deeper; else the line
/// itself (`type A = B;`).
pub(crate) fn item_end(lines: &[&str], start: usize) -> usize {
    let mut depth = 0i64;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start).take(2000) {
        let chars: Vec<char> = line.chars().collect();
        let mut quote: Option<char> = None;
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            if let Some(q) = quote {
                if c == '\\' {
                    k += 1;
                } else if c == q {
                    quote = None;
                }
                k += 1;
                continue;
            }
            match c {
                '"' | '`' => quote = Some(c),
                // A comment's brackets are not the code's.
                '/' if chars.get(k + 1) == Some(&'/') => break,
                // A char literal (`'{'`, `'\\''`), not a lifetime (`'a`).
                '\'' if chars.get(k + 2) == Some(&'\'') => k += 2,
                '\'' if chars.get(k + 1) == Some(&'\\') && chars.get(k + 3) == Some(&'\'') => {
                    k += 3
                }
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
            k += 1;
        }
        if opened && depth <= 0 {
            return i;
        }
        let trimmed = line.trim_end();
        if !opened && i == start {
            if trimmed.ends_with(':') {
                let indent = line.len() - line.trim_start().len();
                let mut last = start;
                for (j, next) in lines.iter().enumerate().skip(start + 1) {
                    if next.trim().is_empty() {
                        continue;
                    }
                    if next.len() - next.trim_start().len() <= indent {
                        break;
                    }
                    last = j;
                }
                return last;
            }
            if trimmed.ends_with(';') {
                return start;
            }
        }
        if !opened && i > start + 3 {
            return start;
        }
    }
    start
}

/// The first line of the doc comments, attributes and decorators right above 0-based `first`.
pub(crate) fn with_leading_docs(lines: &[&str], mut first: usize) -> usize {
    while first > 0 {
        let above = lines[first - 1].trim_start();
        let doc = ["///", "//", "#[", "@", "/*", "*"]
            .iter()
            .any(|prefix| above.starts_with(prefix));
        if !doc {
            break;
        }
        first -= 1;
    }
    first
}

/// Lines `first..=last` (0-based), numbered from 1, at most [`MAX_BODY_LINES`] of them.
pub(crate) fn numbered_lines(lines: &[&str], first: usize, last: usize) -> String {
    let last = last.min(lines.len().saturating_sub(1));
    let shown = last.min(first + MAX_BODY_LINES - 1);
    let width = (shown + 1).to_string().len();
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate().take(shown + 1).skip(first) {
        out.push_str(&format!("{:>width$} | {line}\n", i + 1));
    }
    if shown < last {
        out.push_str(&format!("… {} more line(s)\n", last - shown));
    }
    out.trim_end().to_string()
}
