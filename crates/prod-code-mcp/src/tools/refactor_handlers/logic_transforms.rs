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
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::protocol::McpToolCallResult;
use crate::tools::{compile_gate, refuse_incomplete, resolve_file_path};

pub(crate) async fn handle_generify(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .or_else(|| args.get("file"))
        .and_then(|v| v.as_str());
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("function"))
        .and_then(|v| v.as_str());
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .or_else(|| args.get("col"))
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let param = args
        .get("param")
        .or_else(|| args.get("parameter"))
        .and_then(|v| v.as_str())
        .context("Missing 'param' argument: the parameter to make generic")?;
    let bound = args
        .get("bound")
        .or_else(|| args.get("trait"))
        .or_else(|| args.get("constraint"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let type_param = args
        .get("type_param")
        .or_else(|| args.get("as"))
        .and_then(|v| v.as_str())
        .unwrap_or("T");
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let mut declaration_line = line;

    let file_path = if let Some(p) = path_str {
        resolve_file_path(workspace_root, p)
    } else if let Some(sym) = symbol {
        let wanted = sym
            .rsplit_once("::")
            .map(|(_, name)| name)
            .or_else(|| sym.rsplit_once('.').map(|(_, name)| name))
            .unwrap_or(sym);
        let supported_source = |path: &Path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some(
                    "rs" | "ts"
                        | "tsx"
                        | "js"
                        | "jsx"
                        | "py"
                        | "cpp"
                        | "cc"
                        | "cxx"
                        | "h"
                        | "hpp"
                        | "c"
                        | "swift"
                        | "go"
                        | "java"
                )
            )
        };
        let candidates: std::collections::BTreeSet<(PathBuf, u32, u32)> =
            crate::tools::workspace_symbol_search(remote, workspace_root, sym, None, 100)
                .await?
                .into_iter()
                .filter(|hit| {
                    let name = hit
                        .name
                        .rsplit_once("::")
                        .map(|(_, name)| name)
                        .or_else(|| hit.name.rsplit_once('.').map(|(_, name)| name))
                        .unwrap_or(&hit.name);
                    name.eq_ignore_ascii_case(wanted) && supported_source(&hit.path)
                })
                .map(|hit| {
                    (
                        std::fs::canonicalize(&hit.path).unwrap_or(hit.path),
                        hit.line,
                        hit.col,
                    )
                })
                .collect();
        match candidates.len() {
            0 => anyhow::bail!("no supported declaration for symbol {sym} was found"),
            1 => {
                let (path, found_line, _) = candidates.into_iter().next().unwrap();
                declaration_line = Some(found_line);
                path
            }
            _ => anyhow::bail!(
                "multiple declarations for symbol {sym} were found; pass a path to select one"
            ),
        }
    } else {
        anyhow::bail!("Missing 'path' or 'symbol' argument");
    };

    let done = crate::generify::generify_polyglot(
        remote,
        workspace_root,
        &file_path,
        symbol,
        declaration_line,
        character,
        param,
        bound,
        type_param,
        apply,
        force,
    )
    .await?;

    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else if force {
        McpToolCallResult::text(format!("{text}\n[forced: applied with diagnostics]"))
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_invert_boolean(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("function"))
        .and_then(|v| v.as_str());
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument: the name of the inverted predicate")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";
    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust invert_boolean")?;
        let c = character.context("Missing 'character' argument for Rust invert_boolean")?;
        crate::invert_boolean::invert(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            new_name,
            apply && !verify,
            force,
        )
        .await?
    } else {
        crate::invert_boolean::invert_polyglot(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            symbol,
            new_name,
            apply && !verify,
            force,
        )
        .await?
    };
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.unmatched.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}
