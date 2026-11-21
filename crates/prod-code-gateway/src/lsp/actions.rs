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

#[cfg(test)]
mod tests;

pub(crate) fn range_line(value: &serde_json::Value, end: bool) -> u64 {
    value
        .get(if end { "end" } else { "start" })
        .and_then(|p| p.get("line"))
        .and_then(|l| l.as_u64())
        .unwrap_or(0)
}

/// Stable id of a code action in a list: the server's own `data.id` when it gives one (Rust
/// Intelligence gives rust-analyzer's assist ids, `extract_variable`), else its index plus a
/// slug of the title.
pub(crate) fn code_action_id(index: usize, action: &serde_json::Value) -> String {
    if let Some(id) = action
        .pointer("/data/id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())
    {
        return id.to_string();
    }
    format!("{index}:{}", title_slug(action, 6))
}

/// The title's ASCII words, lowercased and joined by `_`, at most `words` of them.
fn title_slug(action: &serde_json::Value, words: usize) -> String {
    action
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("action")
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
        .take(words)
        .collect::<Vec<_>>()
        .join("_")
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
    let actions = without_equivalent_duplicates(actions.as_array().cloned().unwrap_or_default());
    let ids: Vec<String> = actions
        .iter()
        .enumerate()
        .map(|(i, a)| code_action_id(i, a))
        .collect();
    let variants: Vec<Option<String>> = actions.iter().map(variant_id).collect();
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
                    // Actions sharing a server id (one `auto_import` per import path) are
                    // listed by their variant id, which names the variant itself, so a later
                    // listing that drops or reorders candidates cannot hand over another one.
                    let shared = ids.iter().filter(|id| **id == ids[i]).count() > 1;
                    let id = match (&variants[i], shared) {
                        (Some(variant), true) => variant.clone(),
                        _ => ids[i].clone(),
                    };
                    serde_json::json!({
                        "id": id,
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
            if params.get("subtype").is_some_and(|s| !s.is_null()) {
                anyhow::bail!(
                    "code action `{wanted}`: a subtype does not select a code action of this \
                     language server; pass the id it was listed with"
                );
            }
            // The id as listed: a server id that is unique now, or a variant id. A variant
            // that is gone, or a server id now shared, is refused rather than guessed.
            let matching: Vec<usize> = (0..actions.len())
                .filter(|&i| ids[i] == wanted || variants[i].as_deref() == Some(wanted.as_str()))
                .collect();
            if matching.len() > 1 {
                anyhow::bail!(
                    "code action `{wanted}` is ambiguous at this position: pass one of {}",
                    matching
                        .iter()
                        .map(|&i| format!(
                            "`{}` ({})",
                            variants[i].as_deref().unwrap_or(&ids[i]),
                            actions[i]
                                .get("title")
                                .and_then(|t| t.as_str())
                                .unwrap_or("")
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            let found = matching.first().map(|&i| &actions[i]).or_else(|| {
                // Fall back to the title, so an index id from an older listing still matches.
                let title = wanted.split_once(':').map(|(_, t)| t).unwrap_or(&wanted);
                actions
                    .iter()
                    .enumerate()
                    .find(|(i, a)| {
                        variants[*i].is_none()
                            && code_action_id(*i, a).ends_with(&format!(":{title}"))
                    })
                    .map(|(_, a)| a)
            });
            let Some(action) = found else {
                anyhow::bail!(
                    "no code action `{wanted}` at this position (available: {})",
                    ids.join(", ")
                );
            };
            apply_action(engine, action).await
        }
    }
}

/// The actions with a later copy of an earlier one removed: the same server id and digest, and
/// an edit already in hand, so applying either changes the same text. Lazy actions (no edit
/// yet) are kept as they are, and refused as ambiguous when indistinguishable.
fn without_equivalent_duplicates(actions: Vec<serde_json::Value>) -> Vec<serde_json::Value> {
    let mut seen = std::collections::HashSet::new();
    actions
        .into_iter()
        .filter(|a| {
            let has_edit = a.get("edit").is_some_and(|e| !e.is_null());
            match variant_id(a) {
                Some(variant) if has_edit => seen.insert(variant),
                _ => true,
            }
        })
        .collect()
}

/// `data.id:digest` for an action with a server id: the variant itself, not its place in one
/// listing. The digest covers what the action does — its title, its edit, the diagnostics it
/// fixes (range, code, message) and its command — so two `remove_unnecessary_mut` fixes with
/// one title but different ranges, or two import paths, never share a selector. Request
/// metadata (`data` beyond the id) is left out. Two actions with the same digest do the same.
fn variant_id(action: &serde_json::Value) -> Option<String> {
    let id = action
        .pointer("/data/id")
        .and_then(|v| v.as_str())
        .filter(|id| !id.is_empty())?;
    let diagnostics: Vec<serde_json::Value> = action
        .get("diagnostics")
        .and_then(|d| d.as_array())
        .map(|ds| {
            ds.iter()
                .map(|d| {
                    serde_json::json!({
                        "range": d.get("range"), "code": d.get("code"), "message": d.get("message"),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // The document version in `documentChanges` is the server's count of the text it was sent,
    // not part of what the edit changes: another session opening the same file raises it with
    // the text unchanged. Every URI, range and replacement stays in.
    let mut edit = action
        .get("edit")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if let Some(changes) = edit
        .get_mut("documentChanges")
        .and_then(|c| c.as_array_mut())
    {
        for change in changes {
            if let Some(document) = change
                .get_mut("textDocument")
                .and_then(|d| d.as_object_mut())
            {
                document.remove("version");
            }
        }
    }
    let target = serde_json::json!({
        "title": action.get("title"),
        "edit": edit,
        "diagnostics": diagnostics,
        "command": action.get("command"),
    });
    let digest = xxhash_rust::xxh3::xxh3_64(target.to_string().as_bytes());
    Some(format!("{id}:{digest:016x}"))
}

/// The WorkspaceEdit of one code action: its own edit, its resolved edit, or what its command
/// pushes back.
async fn apply_action(
    engine: &ManagedLsp<'_>,
    action: &serde_json::Value,
) -> anyhow::Result<serde_json::Value> {
    if let Some(edit) = action.get("edit").filter(|e| !e.is_null()) {
        return Ok(edit.clone());
    }
    if action.get("title").is_some() && action.get("command").and_then(|c| c.as_str()).is_none() {
        let resolved = engine.request("codeAction/resolve", action.clone()).await?;
        if let Some(edit) = resolved.get("edit").filter(|e| !e.is_null()) {
            return Ok(edit.clone());
        }
    }
    // A command-only action (clangd refactorings, some tsserver fixes): run it and capture
    // the edit the server pushes back.
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
        .unwrap_or("action")
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

/// Builds an LSP `WorkspaceEdit` (as `documentChanges`) from a refactoring outcome: new files
/// become create operations followed by their content, every rewritten file one whole-file text
/// edit, and file moves rename operations. The analyzer names every path as it was before the
/// refactoring, and LSP applies `documentChanges` in order, so the moves come last, as
/// rust-analyzer's own server sends them: a rewrite after them would name a path a move vacated
/// or gave to another file.
pub(crate) fn workspace_edit_json(
    outcome: &prod_code_engine_rust::RefactorOutcome,
) -> serde_json::Value {
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
