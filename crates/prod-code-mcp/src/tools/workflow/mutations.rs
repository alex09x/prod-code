/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{edits, operation_failure::OperationFailure};
use crate::protocol::McpContentItem;
use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::Path};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewArgs {
    edits: Vec<edits::BlockEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyArgs {
    files: Vec<edits::PreparedFile>,
}

pub(super) async fn invoke(
    placement: &crate::cluster::PlacementStore,
    remote: SocketAddr,
    root: &Path,
    tool: &str,
    args: Value,
) -> Result<Value> {
    match tool {
        "preview" => {
            let request: PreviewArgs = serde_json::from_value(args)?;
            Ok(serde_json::to_value(edits::preview(root, &request.edits)?)?)
        }
        "apply" => apply(root, args).await,
        "verify" => verify(placement, remote, root, args).await,
        _ => bail!("unsupported mutation operation"),
    }
}

fn refused(error: anyhow::Error) -> anyhow::Error {
    OperationFailure {
        error,
        effects: json!({"operation":"apply","status":"refused","writes":0}),
    }
    .into()
}

async fn apply(root: &Path, args: Value) -> Result<Value> {
    let request: ApplyArgs = serde_json::from_value(args).map_err(|error| refused(error.into()))?;
    let _exec = super::super::workspace_exec_lock(root).lock_owned().await;
    let _writer = super::super::workspace_writer::workspace_writer_gate(root)
        .lock_owned()
        .await;
    let _mutation = super::super::execution::workspace_mutation_lock(root)
        .write_owned()
        .await;
    let replacements = request
        .files
        .iter()
        .map(|file| edits::BlockEdit {
            path: file.path.clone(),
            old: file.old.clone(),
            new: file.new.clone(),
        })
        .collect::<Vec<_>>();
    // This pure preflight distinguishes refusals before application from backend errors.
    // The core checks again; neither check coordinates arbitrary external editors.
    let fresh = edits::preview(root, &replacements).map_err(refused)?;
    if fresh.files.len() != request.files.len()
        || fresh
            .files
            .iter()
            .zip(&request.files)
            .any(|(current, expected)| current.base_sha256 != expected.base_sha256)
    {
        return Err(refused(anyhow!(
            "one or more files changed since preview; request a new preview"
        )));
    }
    let touched = edits::apply(root, &request.files).map_err(|error| OperationFailure {
        error,
        effects: json!({"operation":"apply","status":"not_confirmed","writes_possible":true}),
    })?;
    Ok(
        json!({"touched":touched,"effects":{"operation":"apply","status":"applied","affected_files":touched.len()}}),
    )
}

async fn verify(
    placement: &crate::cluster::PlacementStore,
    remote: SocketAddr,
    root: &Path,
    args: Value,
) -> Result<Value> {
    let (tool, args) = super::verification::request(root, args)
        .map_err(|error| super::operation_failure::before("verify", error))?;
    let result = crate::verify::plan::scoped::with_narrow_scope(super::super::execute_tool_with(
        placement.clone(),
        remote,
        root,
        tool,
        args,
    ))
    .await
    .context("verification request failed")
    .map_err(|error| {
        if error.is::<crate::verify::plan::scoped::WorkflowPlanRefusal>() {
            return super::operation_failure::before("verify", error);
        }
        OperationFailure {
            error,
            effects: json!({"operation":"verify","status":"not_confirmed","may_be_running":true}),
        }
        .into()
    })?;
    let mut data = result
        .structured_content
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let text = result
        .content
        .into_iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text,
        })
        .collect::<Vec<_>>()
        .join("\n");
    if result.is_error {
        return Err(OperationFailure {
            error: anyhow!(text),
            effects: json!({"operation":"verify","status":"failed"}),
        }
        .into());
    }
    data.insert("text".into(), json!(text));
    data.insert(
        "effects".into(),
        json!({"operation":"verify","status":"passed"}),
    );
    Ok(Value::Object(data))
}
