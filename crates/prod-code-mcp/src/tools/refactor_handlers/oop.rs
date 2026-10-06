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
use crate::tools::resolve_file_path;

pub(crate) async fn handle_replace_constructor_with_factory(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let type_name = args
        .get("type_name")
        .and_then(|v| v.as_str())
        .context("Missing 'type_name' argument")?;
    let factory_name = args.get("factory_name").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::replace_constructor::replace_constructor_with_factory(
        remote,
        workspace_root,
        &file_path,
        type_name,
        factory_name,
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

pub(crate) async fn handle_replace_constructor_with_builder(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let type_name = args
        .get("type_name")
        .and_then(|v| v.as_str())
        .context("Missing 'type_name' argument")?;
    let builder_name = args.get("builder_name").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::replace_constructor::replace_constructor_with_builder(
        remote,
        workspace_root,
        &file_path,
        type_name,
        builder_name,
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

pub(crate) async fn handle_pull_up(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let class_name = args
        .get("class_name")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str())
        .context("Missing 'class_name' (or `symbol`) argument")?;
    let members: Vec<String> = args
        .get("members")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let target_class = args.get("target_class").and_then(|v| v.as_str());
    let clean_siblings = args
        .get("clean_siblings")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::pull_push::pull_up_impl(
        remote,
        workspace_root,
        &file_path,
        class_name,
        target_class,
        &members,
        clean_siblings,
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

pub(crate) async fn handle_push_down(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let class_name = args
        .get("class_name")
        .or_else(|| args.get("symbol"))
        .and_then(|v| v.as_str())
        .context("Missing 'class_name' (or `symbol`) argument")?;
    let members: Vec<String> = args
        .get("members")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let target_classes: Option<Vec<String>> = args
        .get("target_classes")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        });
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::pull_push::push_down_impl(
        remote,
        workspace_root,
        &file_path,
        class_name,
        target_classes.as_deref(),
        &members,
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

pub(crate) async fn handle_replace_inheritance_with_delegation(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let sub_type = args
        .get("sub_type")
        .or_else(|| args.get("symbol"))
        .or_else(|| args.get("class_name"))
        .and_then(|v| v.as_str())
        .context("Missing 'sub_type' (or `symbol`) argument")?;
    let base_type = args.get("base_type").and_then(|v| v.as_str());
    let field_name = args.get("field_name").and_then(|v| v.as_str());
    let methods: Option<Vec<String>> = args.get("methods").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(str::to_string))
            .collect()
    });
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::replace_inheritance::replace_inheritance_impl(
        remote,
        workspace_root,
        &file_path,
        sub_type,
        base_type,
        field_name,
        methods.as_deref(),
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

pub(crate) async fn handle_replace_conditional_with_polymorphism(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let base_name = args
        .get("base_name")
        .and_then(|v| v.as_str())
        .context("Missing 'base_name' argument")?;
    let method_name = args
        .get("method_name")
        .and_then(|v| v.as_str())
        .context("Missing 'method_name' argument")?;
    let line = args.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let col = args.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let params: Vec<String> = args
        .get("params")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let return_type = args.get("return_type").and_then(|v| v.as_str());
    let target_var = args.get("target_var").and_then(|v| v.as_str());
    let verify = args.get("verify").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);

    let done = crate::replace_conditional::replace_conditional_impl(
        remote,
        workspace_root,
        &file_path,
        line,
        col,
        base_name,
        method_name,
        &params,
        return_type,
        target_var,
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
