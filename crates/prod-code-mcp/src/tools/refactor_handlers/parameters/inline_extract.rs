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

pub(crate) async fn handle_extract_parameter(
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
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new parameter is called")?;
    let ty = args.get("type").and_then(|v| v.as_str());
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    // The compile gate runs `cargo check`, which says nothing about a TypeScript, Python or Go
    // file; letting it pass one would claim a verdict nobody gave.
    anyhow::ensure!(
        !verify
            || crate::extract_parameter::Syntax::of(&file_path)
                == Some(crate::extract_parameter::Syntax::Rust),
        "`verify: compile` runs `cargo check` and is for Rust files; the analyzer's check of \
         the result is reported without it"
    );
    let mut done = crate::extract_parameter::extract(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        ty,
        replace_all,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify {
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
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
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

pub(crate) async fn handle_inline_parameter(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);
    let function = args
        .get("function")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str());
    let param = args
        .get("parameter")
        .or_else(|| args.get("param"))
        .and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";
    anyhow::ensure!(
        !verify || is_rust,
        "verify: compile is only supported for Rust inline_parameter; no files were written"
    );

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust inline_parameter")?;
        let c = character.unwrap_or(1);
        crate::inline_parameter::inline_parameter(
            remote,
            workspace_root,
            &file_path,
            l,
            c,
            apply && !verify,
            force,
        )
        .await?
    } else {
        crate::inline_parameter::inline_parameter_polyglot(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            function,
            param,
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
        && done.unmatched.is_empty()
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
