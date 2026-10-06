/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub(crate) fn display(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

/// Why nothing is rewritten when the analyzer cannot list the references to `name`: with some
/// of them missing, a call or a use would be left behind that the check afterwards may not see
/// (#436).
pub(crate) fn unlisted(name: &str, root: &Path, file: &Path, line: u32, col: u32) -> String {
    format!(
        "the references to `{name}` at {}:{line}:{col} could not be listed, so nothing was \
         rewritten",
        display(root, file)
    )
}

/// The text of a file a reference is in: the declaring `file` as it was read (`text`), any other
/// from disk. One that cannot be read stops the plan: read as empty, its references would be
/// neither rewritten nor reported.
pub(crate) fn read_referenced(path: &Path, file: &Path, text: &str) -> Result<String> {
    if path == file {
        return Ok(text.to_string());
    }
    std::fs::read_to_string(path)
        .with_context(|| format!("cannot read {}; nothing was written", path.display()))
}

/// Type-checks the rewritten files together in one overlay, and writes them when that was asked
/// for and the analyzer accepts them (or `force` says to write them regardless). Returns the
/// errors and whether anything was written. A reference the plan did not rewrite (`unmatched`)
/// stops the write whatever `force` says: `force` overrides the analyzer's verdict on a complete
/// plan, and a call left with the old arguments may still compile (#446).
pub(crate) async fn check_and_apply(
    remote: SocketAddr,
    root: &Path,
    rewritten: &BTreeMap<PathBuf, String>,
    also_check: &[PathBuf],
    unmatched: &[String],
    apply: bool,
    force: bool,
) -> Result<(Vec<String>, bool)> {
    anyhow::ensure!(
        !apply || unmatched.is_empty(),
        "{} reference(s) were not rewritten; nothing was written, and `force` does not override \
         this:\n  {}",
        unmatched.len(),
        unmatched.join("\n  ")
    );
    let to_check: Vec<(PathBuf, String)> = rewritten
        .iter()
        .map(|(p, t)| (p.clone(), t.clone()))
        .collect();
    let reports = crate::diagnostics::validate_texts(remote, root, &to_check, also_check).await?;
    let diagnostics: Vec<String> = reports
        .iter()
        .flat_map(|r| r.items.iter().map(move |d| (r.file.clone(), d)))
        .filter(|(_, d)| d.severity == "error")
        .map(|(f, d)| {
            format!(
                "{}{} ({f}:{}:{})",
                d.message.lines().next().unwrap_or(""),
                d.code
                    .as_deref()
                    .map(|c| format!(" [{c}]"))
                    .unwrap_or_default(),
                d.line,
                d.col
            )
        })
        .collect();

    let mut applied = false;
    if apply {
        anyhow::ensure!(
            diagnostics.is_empty() || force,
            "the change does not compile ({} error(s)); nothing was written. Fix the request, \
             or pass `force: true` to write it anyway:\n  {}",
            diagnostics.len(),
            diagnostics.join("\n  ")
        );
        let edit = crate::signature::whole_file_edit(rewritten);
        crate::refactor::apply_workspace_edit(root, &edit)?;
        applied = true;
    }
    Ok((diagnostics, applied))
}

/// Every reference to the symbol at `file:line:col`, its declarations included. clangd lists a
/// header's prototype and the definition only when it is asked for declarations, so what this
/// has and [`crate::signature::references`] does not are the declarations.
pub(crate) async fn references_with_declarations(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Vec<(PathBuf, u32, u32)>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {:?}", file))?
        .to_string();
    let res = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/references",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) },
            "context": { "includeDeclaration": true },
        }),
    )
    .await?;
    Ok(res
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|loc| {
            let uri = loc.get("uri")?.as_str()?;
            let at = |p: &str| loc.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32 + 1;
            Some((
                PathBuf::from(crate::remote_fs::uri_to_path(uri)),
                at("/range/start/line"),
                at("/range/start/character"),
            ))
        })
        .collect())
}
