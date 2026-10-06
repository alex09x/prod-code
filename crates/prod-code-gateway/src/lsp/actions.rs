/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

pub(crate) fn range_line(value: &serde_json::Value, end: bool) -> u64 {
    value
        .get(if end { "end" } else { "start" })
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0)
}

/// Stable id of a code action in a list: its index plus a slug of the title.
pub(crate) fn code_action_id(index: usize, action: &serde_json::Value) -> String {
    let title = action
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("action");
    let slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|p| !p.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("_");
    format!("{index}:{slug}")
}

/// Lists (`prodCode/assists`) or applies (`prodCode/applyAssist`) LSP code actions on a
/// managed language server. The listing is `[{id, kind, label}]` like the Rust engine's; the
/// apply step re-queries the actions, picks the one with the requested id, resolves it when
/// its edit is lazy and returns the WorkspaceEdit for the client to apply.
pub(crate) async fn lsp_code_actions(
    engine: &ManagedLsp<'_>,
    method: &str,
    params: serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    let uri = params
        .get("textDocument")
        .and_then(|t| t.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let range = params.get("range").cloned().unwrap_or(serde_json::json!({
        "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 }
    }));
    let (from, to) = (range_line(&range, false), range_line(&range, true));
    // Quick fixes are offered for the diagnostics in the requested range; without them the
    // listing would lack its quick fixes and say nothing of it.
    let diagnostics: Vec<serde_json::Value> = engine
        .diagnostics_for(&uri)
        .await
        .map_err(|e| anyhow::anyhow!("code actions need the document's diagnostics: {e}"))?
        .into_iter()
        .filter(|d| {
            d.get("range")
                .map(|r| range_line(r, false) <= to && range_line(r, true) >= from)
                .unwrap_or(false)
        })
        .collect();
    let actions = engine
        .request(
            "textDocument/codeAction",
            serde_json::json!({
                "textDocument": { "uri": uri },
                "range": range,
                "context": { "diagnostics": diagnostics },
            }),
        )
        .await?;
    let actions = actions.as_array().cloned().unwrap_or_default();
    match method {
        "prodCode/assists" => Ok(serde_json::Value::Array(
            actions
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let kind = a.get("kind").and_then(|k| k.as_str()).unwrap_or(
                        if a.get("command").is_some() && a.get("edit").is_none() {
                            "command"
                        } else {
                            "action"
                        },
                    );
                    serde_json::json!({
                        "id": code_action_id(i, a),
                        "kind": kind,
                        "label": a.get("title").and_then(|t| t.as_str()).unwrap_or(""),
                    })
                })
                .collect(),
        )),
        _ => {
            let wanted = params
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let found = actions
                .iter()
                .enumerate()
                .find(|(i, a)| code_action_id(*i, a) == wanted)
                .or_else(|| {
                    // Fall back to the title, so an id from an older listing still matches.
                    let title = wanted.split_once(':').map(|(_, t)| t).unwrap_or(&wanted);
                    actions
                        .iter()
                        .enumerate()
                        .find(|(i, a)| code_action_id(*i, a).ends_with(&format!(":{title}")))
                });
            let Some((_, action)) = found else {
                anyhow::bail!(
                    "no code action `{wanted}` at this position (available: {})",
                    actions
                        .iter()
                        .enumerate()
                        .map(|(i, a)| code_action_id(i, a))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            };
            if let Some(edit) = action.get("edit").filter(|e| !e.is_null()) {
                return Ok(edit.clone());
            }
            if action.get("title").is_some()
                && action.get("command").and_then(|c| c.as_str()).is_none()
            {
                let resolved = engine.request("codeAction/resolve", action.clone()).await?;
                if let Some(edit) = resolved.get("edit").filter(|e| !e.is_null()) {
                    return Ok(edit.clone());
                }
            }
            // A command-only action (clangd refactorings, some tsserver fixes): run it and
            // capture the edit the server pushes back.
            let command = match action.get("command") {
                Some(c) if c.is_object() => c.clone(),
                Some(c) if c.is_string() => serde_json::json!({
                    "command": c,
                    "arguments": action.get("arguments").cloned().unwrap_or(serde_json::json!([])),
                }),
                _ => serde_json::Value::Null,
            };
            let title = action
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or(&wanted)
                .to_string();
            if command.is_null() {
                anyhow::bail!("code action `{title}` carries neither an edit nor a command");
            }
            if let ManagedLsp::Generic(generic) = engine
                && let Some(edit) = generic
                    .execute_command_capturing_edit(command.clone())
                    .await?
            {
                return Ok(edit);
            }
            anyhow::bail!(
                "code action `{title}` ran the server command `{}` without producing an edit",
                command
                    .get("command")
                    .and_then(|c| c.as_str())
                    .unwrap_or("?")
            )
        }
    }
}

/// Builds an LSP `WorkspaceEdit` (as `documentChanges`) from a refactoring outcome: new files
/// become create operations followed by their content, every rewritten file one whole-file text
/// edit, and file moves rename operations. The analyzer names every path as it was before the
/// refactoring, and LSP applies `documentChanges` in order, so the moves come last, as
/// rust-analyzer's own server sends them: a rewrite after them would name a path a move vacated
/// or gave to another file.
pub(crate) fn workspace_edit_json(outcome: &prod_code_engine_rust::RefactorOutcome) -> serde_json::Value {
    let mut changes = Vec::new();
    for created in &outcome.created {
        let uri = file_uri(&created.path);
        changes.push(
            serde_json::json!({ "kind": "create", "uri": uri, "options": { "overwrite": false } }),
        );
        changes.push(serde_json::json!({
            "textDocument": { "uri": uri, "version": null },
            "edits": [ { "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 0 } }, "newText": created.new_text } ]
        }));
    }
    for file in &outcome.files {
        changes.push(serde_json::json!({
            "textDocument": { "uri": file_uri(&file.path), "version": null },
            "edits": [ {
                "range": { "start": { "line": 0, "character": 0 }, "end": { "line": file.old_line_count, "character": 0 } },
                "newText": file.new_text
            } ]
        }));
    }
    for mv in &outcome.moves {
        changes.push(serde_json::json!({
            "kind": "rename",
            "oldUri": file_uri(&mv.from),
            "newUri": file_uri(&mv.to),
            "options": { "overwrite": false }
        }));
    }
    serde_json::json!({ "documentChanges": changes })
}

