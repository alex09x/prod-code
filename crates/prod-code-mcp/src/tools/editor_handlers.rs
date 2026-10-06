/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Code editing and navigation handlers: rename, typecheck, code assists, and safe delete.

use std::net::SocketAddr;
use std::path::Path;

use anyhow::{Context, Result};
use url::Url;

use super::{checked_position_argument, execute_lsp_query, resolve_file_path};
use crate::protocol::McpToolCallResult;

pub(crate) async fn handle_rename(
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
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument")?
        .to_string();
    let file_path = resolve_file_path(workspace_root, path_str);
    if args
        .get("accessors")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        return rename_with_accessors(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            &new_name,
            force,
        )
        .await;
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "newName": new_name
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/rename",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if edit.is_null() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    // The analyzer computes the edit; it does not check that the result compiles. A new name
    // that is already declared in the same scope is renamed into a second definition (#98), so
    // the result is checked in the overlay like every other write, and refused if it breaks.
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let (mut planned, moves_files) = crate::refactor::planned_texts(workspace_root, &edit)?;
    // The old name in comments and test names, in every file the rename touches.
    let comments = args
        .get("comments")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut mentioned = crate::rename_mentions::Mentions::default();
    if comments {
        if moves_files {
            return Ok(McpToolCallResult::error(
                "`comments` is not supported with a rename that moves files; rename first, then \
                 run it again at the new name"
                    .to_string(),
            ));
        }
        let text = std::fs::read_to_string(&file_path).unwrap_or_default();
        // A position on no character names no old name; the file's first word is not one.
        let Some(at) = crate::signature::offset_of(&text, line, character) else {
            return Ok(McpToolCallResult::error(format!(
                "{path_str}:{line}:{character} is not a position in the file, so the old name \
                 in comments cannot be found; nothing was written"
            )));
        };
        let start = text[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let old: String = text[start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !planned.iter().any(|(p, _)| *p == file_path) {
            planned.push((file_path.clone(), text.clone()));
        }
        for (_, t) in planned.iter_mut() {
            let (rewritten, found) = crate::rename_mentions::rewrite(t, &old, &new_name);
            mentioned.comments += found.comments;
            mentioned.tests.extend(found.tests);
            *t = rewritten;
        }
    }
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename to `{new_name}` refused: the result does not compile ({} error(s)); nothing \
             was written. If `{new_name}` is already declared in that scope, pick another name; \
             pass `force: true` to write it anyway:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    // With `comments` the texts are no longer the analyzer's edit alone: write them whole.
    let touched = if comments {
        let files: std::collections::BTreeMap<std::path::PathBuf, String> = planned
            .into_iter()
            .filter(|(p, t)| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true))
            .collect();
        crate::refactor::apply_workspace_edit(
            workspace_root,
            &crate::signature::whole_file_edit(&files),
        )?
    } else {
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?
    };
    let mut text = format!(
        "renamed to `{new_name}`; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    );
    if comments {
        text.push_str(&format!(
            "\n\nin comments: {} mention(s) of the old name replaced",
            mentioned.comments
        ));
        for (from, to) in &mentioned.tests {
            text.push_str(&format!("\ntest renamed: `{from}` -> `{to}`"));
        }
    }
    if !errors.is_empty() {
        text.push_str(&format!(
            "\n\nwritten with `force`, although the analyzer reports {} error(s):\n  {}",
            errors.len(),
            errors.join("\n  ")
        ));
    }
    if moves_files {
        text.push_str("\n\nthe rename also moved files; that part was not checked before writing");
    }
    Ok(McpToolCallResult::text(text))
}

/// A field renamed together with its accessors (#146): every rename merged into one change per
/// file, checked in one overlay, written only when it compiles unless `force`.
async fn rename_with_accessors(
    remote: SocketAddr,
    workspace_root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    new_name: &str,
    force: bool,
) -> Result<McpToolCallResult> {
    let (merged, renamed) = match crate::rename_accessors::plan(
        remote,
        workspace_root,
        file,
        line,
        character,
        new_name,
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if merged.is_empty() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    let planned: Vec<(std::path::PathBuf, String)> =
        merged.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename refused: the result does not compile ({} error(s)); nothing was written:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    let touched = crate::refactor::apply_workspace_edit(
        workspace_root,
        &crate::signature::whole_file_edit(&merged),
    )?;
    Ok(McpToolCallResult::text(format!(
        "renamed {}; {} path(s) updated in the checkout:\n{}",
        renamed.join(", "),
        touched.len(),
        touched.join("\n")
    )))
}

pub(crate) async fn handle_check(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let kind = match tool_name {
        "code_check" => crate::verify::VerifyKind::Check,
        "code_lint" => crate::verify::VerifyKind::Lint,
        "code_benchmarks" => crate::verify::VerifyKind::Bench,
        _ => crate::verify::VerifyKind::Test,
    };
    let filter = args
        .get("filter")
        .or_else(|| args.get("test_filter"))
        .or_else(|| args.get("test"))
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // `path` selects a nested project (any file or directory inside it).
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let env = match args.get("env") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .with_context(|| format!("env `{key}` must be a string, got {value}"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("env must be an object of strings, got {other}"),
    };
    let fix = args.get("fix").and_then(|v| v.as_bool()).unwrap_or(false);
    if fix
        && matches!(
            kind,
            crate::verify::VerifyKind::Check | crate::verify::VerifyKind::Lint
        )
    {
        anyhow::ensure!(
            env.is_empty(),
            "env is not passed to a `fix` run; run without `fix` to set it"
        );
        let fixed = crate::fixit::check_and_fix(
            remote,
            workspace_root,
            hint.as_deref(),
            kind,
            timeout_secs,
        )
        .await?;
        let text = fixed.render(40);
        return Ok(if fixed.ok() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let report = crate::verify::run_verify_with(
        remote,
        workspace_root,
        hint.as_deref(),
        kind,
        filter.as_deref(),
        timeout_secs,
        &env,
        |_| {},
    )
    .await?;
    let text = report.render(40);
    Ok(if report.ok() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

/// rust-analyzer writes a prelude item an assist introduces by its full path — an extracted
/// function returns `std::prelude::v1::Result<T, anyhow::Error>` in a file that imports
/// `anyhow::Result` (#97). On the lines the assist wrote, the path is dropped and the result
/// checked in the overlay; the shorter spelling is used only when the
/// analyzer accepts it, and rust-analyzer's own otherwise. Returns the edit to apply and how
/// many paths were shortened.
async fn prefer_names_in_scope(
    remote: SocketAddr,
    root: &Path,
    edit: serde_json::Value,
) -> Result<(serde_json::Value, usize)> {
    const PRELUDE: &str = "std::prelude::v1::";
    let (planned, moves_files) = crate::refactor::planned_texts(root, &edit)?;
    if moves_files {
        return Ok((edit, 0));
    }
    let mut shortened = 0usize;
    let mut shorter = Vec::with_capacity(planned.len());
    for (path, text) in planned {
        // Only lines the assist wrote: a line that was already in the file keeps its spelling,
        // whatever it says.
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let old_lines: std::collections::HashSet<&str> = before.lines().collect();
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches('\n');
            if body.contains(PRELUDE) && !old_lines.contains(body) {
                shortened += body.matches(PRELUDE).count();
                out.push_str(&line.replace(PRELUDE, ""));
            } else {
                out.push_str(line);
            }
        }
        shorter.push((path, out));
    }
    if shortened == 0 {
        return Ok((edit, 0));
    }
    let reports = crate::diagnostics::validate_texts(remote, root, &shorter, &[]).await?;
    if reports.iter().any(|r| r.errors > 0) {
        return Ok((edit, 0));
    }
    let files: std::collections::BTreeMap<std::path::PathBuf, String> =
        shorter.into_iter().collect();
    Ok((crate::signature::whole_file_edit(&files), shortened))
}

pub(crate) async fn handle_assists(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let (end_line, end_char) = match (
        args.get("end_line").and_then(|v| v.as_u64()),
        args.get("end_character").and_then(|v| v.as_u64()),
    ) {
        (Some(l), Some(c)) => (l as u32, c as u32),
        _ => (line, character),
    };
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let mut params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            "end": { "line": end_line.saturating_sub(1), "character": end_char.saturating_sub(1) }
        }
    });
    if tool_name == "code_assist" {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .context("Missing 'id' argument")?;
        params["id"] = serde_json::json!(id);
        if let Some(subtype) = args.get("subtype").and_then(|v| v.as_u64()) {
            params["subtype"] = serde_json::json!(subtype);
        }
        let edit = match execute_lsp_query(
            remote,
            workspace_root,
            &file_path,
            "prodCode/applyAssist",
            params,
        )
        .await
        {
            Ok(edit) => edit,
            Err(e) => {
                return Ok(McpToolCallResult::error(format!("assist refused: {e:#}")));
            }
        };
        let (edit, respelled) = prefer_names_in_scope(remote, workspace_root, edit).await?;
        let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        let mut text = format!(
            "applied `{id}`; {} path(s) updated in the checkout:\n{}",
            touched.len(),
            touched.join("\n")
        );
        if respelled > 0 {
            text.push_str(&format!(
                "\n\n{respelled} `std::prelude::v1::` path(s) the assist wrote are spelled as the \
                 name already in scope; the analyzer accepts the shorter spelling"
            ));
        }
        return Ok(McpToolCallResult::text(text));
    }
    let list = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/assists",
        params,
    )
    .await?;
    let mut out = String::new();
    if let Some(items) = list.as_array() {
        if items.is_empty() {
            out.push_str("no code actions at this position\n");
        }
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
            match item.get("subtype").and_then(|v| v.as_u64()) {
                Some(st) => out.push_str(&format!("{id} (subtype {st}) [{kind}]: {label}\n")),
                None => out.push_str(&format!("{id} [{kind}]: {label}\n")),
            }
        }
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}

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
