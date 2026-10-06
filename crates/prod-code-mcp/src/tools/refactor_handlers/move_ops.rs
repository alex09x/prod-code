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
use crate::tools::{compile_gate, refuse_incomplete, resolve_file_path};

pub(crate) async fn handle_move(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: the target module's file")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let target = resolve_file_path(workspace_root, to);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    anyhow::ensure!(
        !verify || ext == "rs",
        "`verify: compile` runs `cargo check` and is for Rust files; the analyzer's check of \
         the result is reported without it"
    );
    let mut moved = crate::move_item::move_item(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &target,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &moved.unmatched)?;
    let gate = if verify {
        let files = moved.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                moved.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        moved.applied = true;
    }
    let clean = moved.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = moved.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_move_method(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file = resolve_file_path(workspace_root, path_str);
    let (line, character) = (num("line")?, num("character")?);
    let done = match (
        args.get("to_param").and_then(|v| v.as_str()),
        args.get("to_type").and_then(|v| v.as_str()),
    ) {
        (Some(to_param), None) => {
            crate::move_method::move_method(
                remote,
                workspace_root,
                &file,
                line,
                character,
                to_param,
                apply,
                force,
            )
            .await?
        }
        (None, Some(to_type)) => {
            crate::move_method::move_associated_function(
                remote,
                workspace_root,
                &file,
                line,
                character,
                to_type,
                apply,
                force,
            )
            .await?
        }
        _ => anyhow::bail!(
            "give `to_param` (a method: the parameter whose type it moves to) or `to_type` (an \
             associated function: the type it moves to), one of them"
        ),
    };
    let text = done.render(8000);
    let clean = done.diagnostics.is_empty() && done.blocked.is_empty() && done.unmatched.is_empty();
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_move_module(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument: the module's file")?;
    let to_str = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: where the module's file goes")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let mut done = crate::move_module::move_module(
        remote,
        workspace_root,
        &resolve_file_path(workspace_root, path_str),
        &resolve_file_path(workspace_root, to_str),
    )
    .await?;
    let gate = if verify {
        Some(
            compile_gate(
                remote,
                workspace_root,
                &done.rewritten,
                done.diagnostics.is_empty(),
                false,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    let compiles = gate.as_ref().is_none_or(|g| g.passed);
    // The gate only judges: the move also deletes the files it left, which only `write` does.
    if apply && (compiles || force) {
        done.write(force)?;
    }
    let clean = done.diagnostics.is_empty() && compiles;
    let mut text = done.render(8000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
        if apply && !compiles && !force {
            text.push_str("\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n");
        }
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}
