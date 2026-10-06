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
use url::Url;

use crate::protocol::McpToolCallResult;
use crate::tools::{checked_position_argument, execute_lsp_query, resolve_file_path};

pub(crate) async fn handle_safe_delete(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line =
        checked_position_argument(args.get("line").context("Missing 'line' argument")?, "line")?;
    let character = checked_position_argument(
        args.get("character")
            .context("Missing 'character' argument")?,
        "character",
    )?;
    let file_path = resolve_file_path(workspace_root, path_str);
    // Go has no analyzer safe-delete request. Its narrow compiler-verified planner must run
    // before the Rust parameter scanner can mistake Go syntax for a parameter.
    if file_path
        .extension()
        .is_some_and(|extension| extension == "go")
    {
        return Ok(
            match crate::safe_delete_go::delete_function(
                remote,
                workspace_root,
                &file_path,
                line,
                character,
            )
            .await
            {
                Ok(deleted) => McpToolCallResult::text(format!(
                    "deleted unreferenced Go function {}; compiler-verified under the active Go build flags; 1 path updated:\n{}",
                    deleted.name,
                    deleted.path.display()
                )),
                Err(error) => McpToolCallResult::error(format!("safe delete refused: {error:#}")),
            },
        );
    }
    if file_path
        .extension()
        .is_some_and(|extension| extension == "ts")
    {
        return Ok(
            match crate::safe_delete_typescript::delete_function(
                remote,
                workspace_root,
                &file_path,
                line,
                character,
            )
            .await
            {
                Ok(deleted) => McpToolCallResult::text(format!(
                    "deleted unreferenced private TypeScript function {}; compiler-verified under the active TypeScript configuration; 1 path updated:\n{}",
                    deleted.name,
                    deleted.path.display()
                )),
                Err(error) => McpToolCallResult::error(format!("safe delete refused: {error:#}")),
            },
        );
    }
    // A parameter goes from the declaration and from every call at once, through
    // `change_signature`, which refuses while the body still uses it.
    let text = std::fs::read_to_string(&file_path).unwrap_or_default();
    if let Some((fn_at, name, kept)) = crate::signature::parameter_at(&text, line, character) {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        // A trait method's parameter goes from the trait, every implementation and every call,
        // by its position (#194).
        if crate::trait_param::owner_of(&text, fn_at).is_some() {
            let (_, open, close) = crate::signature::param_span(&text, fn_at)
                .context("the method has no parameter list")?;
            let (_, declared) = crate::signature::parse_declared(&text[open..close]);
            let index = declared
                .iter()
                .position(|d| d.name == name)
                .context("the parameter is not in the method's list")?;
            let done = crate::trait_param::remove_parameter(
                remote,
                workspace_root,
                &file_path,
                fn_at,
                index,
                true,
                force,
            )
            .await
            .with_context(|| format!("safe delete of the parameter `{name}` refused"))?;
            let text = done.render(6000);
            return Ok(if done.applied && done.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            });
        }
        let request = kept
            .iter()
            .map(|k| crate::signature::parse_param(k))
            .collect::<Result<Vec<_>>>()?;
        let (fl, fc) = crate::signature::position_at(&text, fn_at)?;
        let change = crate::signature::change(
            remote,
            workspace_root,
            &file_path,
            fl,
            fc,
            &request,
            true,
            force,
        )
        .await
        .with_context(|| format!("safe delete of the parameter `{name}` refused"))?;
        let text = format!(
            "the parameter `{name}` is removed, with its argument at every call\n\n{}",
            change.render(6000)
        );
        return Ok(if change.diagnostics.is_empty() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/safeDelete",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => {
            return Ok(McpToolCallResult::error(format!(
                "safe delete refused: {e:#}"
            )));
        }
    };
    let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
    // An answer with no edit is not a deletion: saying "deleted" would be a success that did
    // nothing (#138).
    if touched.is_empty() {
        return Ok(McpToolCallResult::error(
            "safe delete produced no edit; nothing was deleted".to_string(),
        ));
    }
    Ok(McpToolCallResult::text(format!(
        "deleted; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    )))
}
