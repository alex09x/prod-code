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
use crate::tools::{
    compile_gate, execute_lsp_query, representative_source_file, resolve_file_path, rewritten_files,
};

pub(crate) async fn handle_schema_rename(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let field = args
        .get("field")
        .and_then(|v| v.as_str())
        .context("Missing 'field' argument")?;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let workspace_edit = args
        .get("workspace_edit")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let scope = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let repos = match args.get("repos") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .map(|v| {
                let path = v
                    .as_str()
                    .with_context(|| format!("repos takes paths, got {v}"))?;
                let path = resolve_file_path(workspace_root, path);
                std::fs::canonicalize(&path)
                    .with_context(|| format!("repository {} cannot be read", path.display()))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("repos takes a list of paths, got {other}"),
    };
    if !repos.is_empty() {
        anyhow::ensure!(
            scope.is_none() && !verify,
            "`path` and `verify` narrow or check one repository; drop them to rename across `repos`"
        );
        let mut roots = vec![workspace_root.to_path_buf()];
        roots.extend(repos);
        let done = crate::schema::rename_across(remote, &roots, field, to, apply, force).await?;
        if workspace_edit {
            let json = serde_json::to_string_pretty(&done.workspace_edit())?;
            return Ok(if done.clean() {
                McpToolCallResult::text(json)
            } else {
                McpToolCallResult::error(json)
            });
        }
        let text = done.render(6000);
        return Ok(if done.clean() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let mut done = crate::schema::rename(
        remote,
        workspace_root,
        field,
        to,
        apply && !verify,
        force,
        scope.as_deref(),
    )
    .await?;
    let gate = if verify {
        let files = done
            .rewritten
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect::<Vec<_>>();
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
    if workspace_edit {
        let json = serde_json::to_string_pretty(&done.workspace_edit())?;
        return Ok(if clean {
            McpToolCallResult::text(json)
        } else {
            McpToolCallResult::error(json)
        });
    }
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

pub(crate) async fn handle_codemod(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let rule = args
        .get("rule")
        .and_then(|v| v.as_str())
        .context("Missing 'rule' argument")?;
    if !rule.contains("==>>") {
        return Ok(McpToolCallResult::error(
            "a rule is `pattern ==>> replacement`, for example `$a.unwrap() ==>> $a.expect(\"invariant\")`"
                .to_string(),
        ));
    }
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    // `path` restricts the rewrite to one file and doubles as the resolve context.
    let scope = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| crate::codemod::resolve_workspace_scope(workspace_root, p))
        .transpose()?;

    // First: Run polyglot structural AST codemod engine across target scope.
    let outcome = crate::codemod::run_codemod(workspace_root, rule, scope.as_deref(), apply)?;
    if outcome.files_matched > 0 {
        let mut text = format!("`{rule}`\n");
        text.push_str(&format!(
            "{} changed line(s) in {} file(s)\n\n",
            outcome.changed_lines, outcome.files_matched
        ));
        const MAX_DIFF: usize = 6000;
        if outcome.diff.len() > MAX_DIFF {
            let cut: String = outcome.diff.chars().take(MAX_DIFF).collect();
            text.push_str(&cut);
            text.push_str("\n… diff truncated\n");
        } else {
            text.push_str(&outcome.diff);
        }
        if apply {
            let written_paths: Vec<String> = outcome
                .rewritten_files
                .iter()
                .map(|(p, _)| {
                    p.strip_prefix(workspace_root)
                        .map(|r| r.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| p.to_string_lossy().into_owned())
                })
                .collect();
            text.push_str(&format!(
                "\n[applied to {} file(s): {}]\n",
                written_paths.len(),
                written_paths.join(", ")
            ));
        } else {
            text.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
        }
        return Ok(McpToolCallResult::text(text.trim_end().to_string()));
    }

    // Fallback: If polyglot AST matched 0 files, check if there's a Rust file context to query rust-analyzer SSR
    let is_rust_target = match &scope {
        Some(p) => p.extension().is_some_and(|ext| ext == "rs"),
        None => true,
    };

    if is_rust_target {
        let context = match scope.clone() {
            Some(p) => Some(p),
            None => representative_source_file(workspace_root),
        };
        if let Some(context) = context
            && let Ok(uri) = Url::from_file_path(&context)
            && let Ok(edit) = execute_lsp_query(
                remote,
                workspace_root,
                &context,
                "prodCode/structuralReplace",
                serde_json::json!({
                    "rule": rule,
                    "scope": scope.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    "textDocument": { "uri": uri.to_string() },
                    "position": { "line": 0, "character": 0 },
                }),
            )
            .await
        {
            let rewritten = rewritten_files(&edit);
            if !rewritten.is_empty() {
                let mut text = format!("`{rule}`\n");
                let mut changed_lines = 0usize;
                let mut body = String::new();
                for (path, new_text) in &rewritten {
                    let rel = std::path::Path::new(path)
                        .strip_prefix(workspace_root)
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|_| path.clone());
                    let old_text = crate::refactor::text_before_apply(Path::new(path));
                    let diff = similar::TextDiff::from_lines(&old_text, new_text);
                    let file_changed = diff
                        .iter_all_changes()
                        .filter(|c| c.tag() != similar::ChangeTag::Equal)
                        .count();
                    changed_lines += file_changed;
                    body.push_str(
                        &diff
                            .unified_diff()
                            .context_radius(2)
                            .header(&format!("a/{rel}"), &format!("b/{rel}"))
                            .to_string(),
                    );
                }
                text.push_str(&format!(
                    "{} changed line(s) in {} file(s)\n\n",
                    changed_lines,
                    rewritten.len()
                ));
                const MAX_DIFF: usize = 6000;
                if body.len() > MAX_DIFF {
                    let cut: String = body.chars().take(MAX_DIFF).collect();
                    text.push_str(&cut);
                    text.push_str("\n… diff truncated\n");
                } else {
                    text.push_str(&body);
                }
                if apply {
                    let written = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
                    text.push_str(&format!(
                        "\n[applied to {} file(s): {}]\n",
                        written.len(),
                        written.join(", ")
                    ));
                } else {
                    text.push_str(
                        "\nnothing was written; pass `apply: true` to make these edits\n",
                    );
                }
                return Ok(McpToolCallResult::text(text.trim_end().to_string()));
            }
        }
    }

    Ok(McpToolCallResult::text(format!(
        "`{rule}` matches nothing{}",
        match &scope {
            Some(p) => format!(" in {}", p.display()),
            None => " in this workspace".to_string(),
        }
    )))
}
