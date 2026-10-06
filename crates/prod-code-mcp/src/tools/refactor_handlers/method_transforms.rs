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

pub(crate) async fn handle_make_static(
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
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust make_static")?;
        let c = character.unwrap_or(1);
        crate::make_static::make_static(
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
        let symbol = args.get("symbol").and_then(|v| v.as_str());
        let method_arg = args
            .get("method")
            .or_else(|| args.get("method_name"))
            .and_then(|v| v.as_str());
        let class_arg = args
            .get("class_name")
            .or_else(|| args.get("struct_name"))
            .and_then(|v| v.as_str());

        let (resolved_class, resolved_method) = if let Some(m) = method_arg {
            (class_arg.map(str::to_string), m.to_string())
        } else if let Some(s) = symbol {
            if let Some((cls, mth)) = s.split_once("::").or_else(|| s.split_once('.')) {
                (Some(cls.to_string()), mth.to_string())
            } else {
                (class_arg.map(str::to_string), s.to_string())
            }
        } else if let Some(l) = line {
            let text = std::fs::read_to_string(&file_path)?;
            let (mth, cls) = crate::make_static::find_method_at_line(&text, l)
                .context("Could not find method at given line")?;
            (class_arg.map(str::to_string).or(cls), mth)
        } else {
            anyhow::bail!("Missing 'method', 'symbol', or line position");
        };

        crate::make_static::make_static_polyglot(
            remote,
            workspace_root,
            &file_path,
            resolved_class.as_deref(),
            &resolved_method,
            apply && !verify,
            force,
        )
        .await?
    };

    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && done.blocked.is_empty() {
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

pub(crate) async fn handle_convert_to_method(
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
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let ext = file_path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = ext == "rs";

    let mut done = if is_rust {
        let l = line.context("Missing 'line' argument for Rust convert_to_method")?;
        let c = character.unwrap_or(1);
        crate::to_method::convert_to_method(
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
        let symbol = args.get("symbol").and_then(|v| v.as_str());
        let method_arg = args
            .get("method")
            .or_else(|| args.get("method_name"))
            .and_then(|v| v.as_str());
        let class_arg = args
            .get("class_name")
            .or_else(|| args.get("struct_name"))
            .and_then(|v| v.as_str());

        let (resolved_class, resolved_method) = if let Some(m) = method_arg {
            (class_arg.map(str::to_string), m.to_string())
        } else if let Some(s) = symbol {
            if let Some((cls, mth)) = s.split_once("::").or_else(|| s.split_once('.')) {
                (Some(cls.to_string()), mth.to_string())
            } else {
                (class_arg.map(str::to_string), s.to_string())
            }
        } else if let Some(l) = line {
            let text = std::fs::read_to_string(&file_path)?;
            let (mth, cls) = crate::make_static::find_method_at_line(&text, l)
                .context("Could not find method at given line")?;
            (class_arg.map(str::to_string).or(cls), mth)
        } else {
            anyhow::bail!("Missing 'method', 'symbol', or line position");
        };

        crate::to_method::convert_to_method_polyglot(
            remote,
            workspace_root,
            &file_path,
            resolved_class.as_deref(),
            &resolved_method,
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

pub(crate) async fn handle_wrap_return(
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
    let wrapper = crate::wrap_return::Wrapper::parse(
        args.get("wrapper")
            .and_then(|v| v.as_str())
            .context("Missing 'wrapper' argument: `option`, `result`, `promise`, `pointer`, or custom envelope")?,
    )?;
    let constructor = args.get("constructor").and_then(|v| v.as_str());
    let error = args.get("error").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");

    let file_path = if let Some(p) = path_str {
        resolve_file_path(workspace_root, p)
    } else if let Some(sym) = symbol {
        let mut found = None;
        for entry in ignore::WalkBuilder::new(workspace_root).build().flatten() {
            let p = entry.path();
            if p.is_file()
                && let Ok(content) = std::fs::read_to_string(p)
                && content.contains(sym)
            {
                found = Some(p.to_path_buf());
                break;
            }
        }
        found.with_context(|| format!("could not find file declaring symbol `{sym}`"))?
    } else {
        anyhow::bail!("Missing 'path' or 'symbol' argument");
    };

    let mut done = crate::wrap_return::wrap_polyglot_ext(
        remote,
        workspace_root,
        &file_path,
        symbol,
        line,
        character,
        wrapper,
        constructor,
        error,
        apply && !verify,
        force,
    )
    .await?;
    refuse_incomplete(apply, &done.unmatched)?;
    let gate = if verify && (done.blocked.is_empty() || force) {
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
