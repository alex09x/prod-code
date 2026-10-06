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
use crate::tools::{compile_gate, resolve_file_path};

pub(crate) async fn handle_extract_delegate(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .or_else(|| args.get("class").and_then(|v| v.as_str()))
        .or_else(|| args.get("type").and_then(|v| v.as_str()));
    let line = args.get("line").and_then(|v| v.as_u64()).map(|v| v as u32);
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .map(|v| v as u32);

    if symbol.is_none() && line.is_none() {
        anyhow::bail!(
            "Specify either `symbol` (or `class`/`type`) or `line` and `character` for the class/struct"
        );
    }

    let list = |key: &str| -> Vec<String> {
        args.get(key)
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let text = |key: &str| -> Result<String> {
        args.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let fields = list("fields");
    anyhow::ensure!(!fields.is_empty(), "Missing or empty 'fields' argument");
    let methods = list("methods");
    let helper_name = text("name")?;
    let field_name = text("field")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");

    let file_path = resolve_file_path(workspace_root, path_str);
    anyhow::ensure!(
        !verify
            || crate::parameter_object::Language::of(&file_path)
                == Some(crate::parameter_object::Language::Rust),
        "`verify: compile` runs `cargo check`, which judges Rust only; {} is checked by its \
         language server's diagnostics alone",
        path_str
    );

    let mut done = crate::extract_delegate::extract_delegate_polyglot(
        remote,
        workspace_root,
        &file_path,
        symbol,
        line,
        character,
        &fields,
        &methods,
        &helper_name,
        &field_name,
        apply && !verify,
        force,
        None,
    )
    .await?;

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
    let mut out = done.render();
    if let Some(gate) = &gate {
        out.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(out)
    } else {
        McpToolCallResult::error(out)
    })
}

pub(crate) async fn handle_extract_trait(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let methods: Vec<String> = args
        .get("methods")
        .and_then(|v| v.as_array())
        .context("Missing 'methods' argument")?
        .iter()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect();
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let migrate_callers = args
        .get("migrate_callers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::extract_trait::extract_trait_ext(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        &methods,
        name,
        migrate_callers,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_interface(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let symbol = args
        .get("symbol")
        .or_else(|| args.get("type_name"))
        .and_then(|v| v.as_str())
        .context("Missing 'symbol' argument")?;
    let interface_name = args
        .get("interface_name")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .context("Missing 'interface_name' argument")?;
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let methods: Vec<String> = args
        .get("methods")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let col = args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    if file_path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
        anyhow::ensure!(
            line > 0 && col > 0,
            "Rust extract_interface requires one-based line and character positions"
        );
    }
    let migrate_callers = args
        .get("migrate_callers")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);

    let done = crate::extract_interface::extract_interface_impl(
        remote,
        workspace_root,
        &file_path,
        symbol,
        interface_name,
        &methods,
        line,
        col,
        migrate_callers,
        apply,
        force,
        verify,
    )
    .await?;

    let text = done.render(2048);
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_extract_function(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let duplicates = args
        .get("duplicates")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let flag = |key: &str| args.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    let apply = flag("apply");
    let force = flag("force");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::extract_function::extract_function(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        duplicates,
        flag("parameterize"),
        flag("other_files"),
    )
    .await?;
    // rust-analyzer does not check borrows: a duplicate whose call moves a value the code after
    // it still uses type-checks and does not compile. So the compiler sees any such result.
    let is_rust = file_path.extension().and_then(|e| e.to_str()) == Some("rs");
    let verify = (args.get("verify").and_then(|v| v.as_str()) == Some("compile")
        || done.replaced() > 0)
        && is_rust;
    let gate = if verify {
        Some(
            compile_gate(
                remote,
                workspace_root,
                &done.rewritten,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        if apply {
            done.write(force)?;
        }
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render();
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

pub(crate) async fn handle_introduce_variable(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::introduce_variable::introduce_variable(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}
