/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use std::net::SocketAddr;
use std::path::Path;

pub(crate) fn resolve_file_path(workspace_root: &Path, path_str: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(path_str);
    if p.is_absolute() {
        if p.exists() {
            p
        } else {
            let rel = path_str.trim_start_matches('/');
            let in_ws = workspace_root.join(rel);
            if in_ws.exists() || !p.starts_with(workspace_root) {
                in_ws
            } else {
                p
            }
        }
    } else {
        workspace_root.join(p)
    }
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
pub async fn execute_lsp_query(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    // One long-lived session per checkout for the life of this process (see
    // crate::session::pooled_query): local edits are pushed before the query.
    crate::session::pooled_query(remote, workspace_root, file_path, method, params).await
}

/// Refuses to write a plan that left references it did not rewrite. With `verify: "compile"` the
/// planner runs as a dry run and the compile gate writes its files, so its own refusal never
/// comes; the gate judges compilation, and neither it nor `force` completes a plan (#446).
pub(crate) fn refuse_incomplete(apply: bool, unmatched: &[String]) -> Result<()> {
    anyhow::ensure!(
        !apply || unmatched.is_empty(),
        "{} reference(s) were not rewritten; nothing was written:\n  {}",
        unmatched.len(),
        unmatched.join("\n  ")
    );
    Ok(())
}

/// What asking the compiler added to a write tool's run.
pub(crate) struct CompileGate {
    pub(crate) text: String,
    pub(crate) passed: bool,
    pub(crate) applied: bool,
}

/// `verify: "compile"`. The tool has built its edit without writing it; the compiler judges it in
/// a shadow of the workspace, and only a result both the analyzer and the compiler accept is
/// written. The overlay check alone does not see an unresolved type (#63), which is precisely
/// what a tool that creates or moves a name can produce.
pub(crate) async fn compile_gate(
    remote: SocketAddr,
    root: &Path,
    files: &[(String, String)],
    analyzer_clean: bool,
    apply: bool,
    force: bool,
) -> Result<CompileGate> {
    if !analyzer_clean && !force {
        return Ok(CompileGate {
            text: "\nthe compiler was not asked: the analyzer already rejects the result\n".into(),
            passed: false,
            applied: false,
        });
    }
    let verdict = crate::compile_check::check(remote, root, files).await?;
    let mut text = verdict.render();
    let mut applied = false;
    if apply {
        if verdict.passed || force {
            let files: std::collections::BTreeMap<std::path::PathBuf, String> = files
                .iter()
                .map(|(p, t)| (std::path::PathBuf::from(p), t.clone()))
                .collect();
            crate::refactor::apply_workspace_edit(
                root,
                &crate::signature::whole_file_edit(&files),
            )?;
            applied = true;
        } else {
            text.push_str(
                "\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n",
            );
        }
    }
    Ok(CompileGate {
        text,
        passed: verdict.passed,
        applied,
    })
}
