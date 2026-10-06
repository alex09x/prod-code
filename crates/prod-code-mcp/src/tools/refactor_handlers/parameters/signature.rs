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

pub(crate) async fn handle_change_signature(
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
    let specs = args
        .get("params")
        .and_then(|v| v.as_array())
        .context("Missing 'params' argument: the parameter list the function should end up with")?;
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        let spec = spec
            .as_str()
            .context("every entry of `params` is a string: `name`, or `name: Type = expression`")?;
        params.push(crate::signature::parse_param(spec)?);
    }
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    // The compile gate runs `cargo check` on a preview and writes it on the compiler's word
    // alone, past the Go adapter's own refusals of a gopls edit that is not the signature change
    // asked for. Go gets no such gate: `verify` is refused before anything is planned.
    if file_path.extension().is_some_and(|e| e == "go")
        && let Some(asked) = args.get("verify").filter(|v| !v.is_null())
    {
        anyhow::bail!(
            "`verify: {asked}` is not supported for Go: it runs `cargo check`, which does not \
             build Go, and nothing was written. A Go signature change is already type-checked with every \
             package that uses it and refused unless gopls's edit is exactly the requested \
             parameter list; omit `verify`"
        );
    }
    anyhow::ensure!(
        !verify || ext == "rs",
        "verify: compile for change_signature is supported only for Rust; the gateway compile gate runs cargo check and nothing was written"
    );
    let modifiers = crate::signature::Modifiers {
        returns: args
            .get("returns")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        visibility: args
            .get("visibility")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        asyncness: args.get("async").and_then(|v| v.as_bool()),
    };
    let mut change = crate::signature::change_with(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &params,
        &modifiers,
        apply && !verify,
        force,
    )
    .await?;
    // With `verify` the planner ran as a dry run and the compile gate writes; a reorder that
    // compiles can still run differently through a reference it did not rewrite (#446).
    if apply {
        change.ensure_writable(force)?;
    }
    let gate = if verify {
        let files = change.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                change.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        change.applied = true;
    }
    let clean = change.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = change.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_introduce_parameter_object(
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
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new struct is called")?;
    let specs = args
        .get("params")
        .and_then(|v| v.as_array())
        .context("Missing 'params' argument: the parameters to bundle, by name")?;
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        params.push(
            spec.as_str()
                .context("every entry of `params` is a parameter name")?
                .to_string(),
        );
    }
    let file_path = resolve_file_path(workspace_root, path_str);
    let binding = args
        .get("binding")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| crate::parameter_object::default_binding(&file_path, name));
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    // The compile gate is `cargo check`; it has nothing to say about another language, and a
    // pass from it would be read as a verdict on files it never compiled.
    anyhow::ensure!(
        !verify
            || crate::parameter_object::Language::of(&file_path)
                == Some(crate::parameter_object::Language::Rust),
        "`verify: compile` runs `cargo check`, which judges Rust only; {} is checked by its \
         language server's diagnostics alone",
        path_str
    );
    let mut done = crate::parameter_object::introduce(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &params,
        name,
        &binding,
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
    let clean = done.unmatched.is_empty()
        && done.diagnostics.is_empty()
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
