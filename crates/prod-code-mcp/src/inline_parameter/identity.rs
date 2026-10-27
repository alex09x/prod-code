/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What a named argument is, wherever a call passes it (#1508). `LIMIT` passed from two modules
//! can be two constants, and the callee's own `LIMIT` a third: the text alone does not say.

use anyhow::{Context, Result};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Where the analyzer says a name is declared: its file and its 0-based line and column.
type Declaration = (PathBuf, u32, u32);

/// The text to bind in the callee for an argument every call spells `arg`: the analyzer must
/// place each call's `arg` at one declaration. A bare name declared in the workspace is written
/// as the absolute path to it (`crate::a::LIMIT`), so it means the same in the callee's module;
/// a path, or a name declared outside the workspace (`None`, a dependency's item), is kept as
/// written. `names` gives, per call, the file, its text and the byte offset of the argument's
/// last path segment.
pub(crate) async fn bound_text(
    remote: SocketAddr,
    root: &Path,
    arg: &str,
    names: &[(PathBuf, String, usize)],
    callee: (&Path, u32),
) -> Result<String> {
    let mut seen: Option<Declaration> = None;
    for (path, text, at) in names {
        let (line, col) = crate::signature::position_at(text, *at)?;
        let declaration = declaration_at(remote, root, path, line, col)
            .await
            .with_context(|| {
                format!(
                    "the analyzer does not say what `{arg}` is at {}:{line}:{col}; nothing was \
                     written",
                    super::types::display(root, path)
                )
            })?;
        match &seen {
            None => seen = Some(declaration),
            Some(first) if *first == declaration => {}
            Some(first) => anyhow::bail!(
                "the calls pass `{arg}`, but it is not one item: {}:{} and {}:{}; nothing was \
                 written",
                super::types::display(root, &first.0),
                first.1 + 1,
                super::types::display(root, &declaration.0),
                declaration.1 + 1
            ),
        }
    }
    let (file, line, _) = seen.context("no call names the argument")?;
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let file = std::fs::canonicalize(&file).unwrap_or(file);
    // A path (`Mode::Fast`, `crate::limits::MAX`) is kept as the calls write it once they agree;
    // a bare name is what resolves differently in another module.
    if arg.contains("::") || !file.starts_with(&root) {
        return Ok(arg.to_string());
    }
    let (declared_in, spelled) = module_path(remote, &root, &file, line).await?;
    let callee_file = std::fs::canonicalize(callee.0).unwrap_or_else(|_| callee.0.to_path_buf());
    // Declared in the callee's own module, the name means the same there as written.
    if module_path(remote, &root, &callee_file, callee.1).await?.0 == declared_in {
        return Ok(arg.to_string());
    }
    anyhow::ensure!(
        spelled,
        "every call passes `{arg}`, declared in {} outside the callee's module, and no path to it \
         can be written (no Cargo.toml above it); nothing was written",
        super::types::display(&root, &file)
    );
    Ok(declared_in
        .into_iter()
        .chain(std::iter::once(arg.trim().to_string()))
        .collect::<Vec<_>>()
        .join("::"))
}

/// The module that the 0-based `line` of `file` is in, spelled from the crate root
/// (`["crate", "a", "b"]`: the file's module and the inline `mod` blocks around the line), and
/// whether that spelling is a path. Without a crate to place the file in, the file stands for
/// its module: enough to tell two modules apart, not to name one.
async fn module_path(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
) -> Result<(Vec<String>, bool)> {
    let inline = inline_modules(remote, root, file, line).await?;
    let (head, spelled) = match crate::move_item::module_of(file) {
        Ok((_, module)) => (
            std::iter::once("crate".to_string())
                .chain(module.segments)
                .collect::<Vec<_>>(),
            true,
        ),
        Err(_) => (vec![file.to_string_lossy().into_owned()], false),
    };
    Ok((head.into_iter().chain(inline).collect(), spelled))
}

/// The analyzer's declaration of the name at the 1-based `line`:`col` of `file`.
async fn declaration_at(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
    col: u32,
) -> Result<Declaration> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let answer = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/definition",
        serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line - 1, "character": col - 1 },
        }),
    )
    .await?;
    let found = answer
        .as_array()
        .and_then(|all| all.first())
        .or_else(|| answer.get("uri").map(|_| &answer))
        .context("no declaration")?;
    let uri = found
        .get("uri")
        .or_else(|| found.get("targetUri"))
        .and_then(|u| u.as_str())
        .context("the declaration has no file")?;
    let start = found
        .pointer("/range/start")
        .or_else(|| found.pointer("/targetSelectionRange/start"))
        .context("the declaration has no position")?;
    let path = url::Url::parse(uri)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .context("the declaration is not a file")?;
    let at = |key: &str| start.get(key).and_then(|v| v.as_u64()).map(|v| v as u32);
    Ok((path, at("line").unwrap_or(0), at("character").unwrap_or(0)))
}

/// The inline `mod` blocks of `file` that enclose its 0-based `line`, outermost first.
async fn inline_modules(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    line: u32,
) -> Result<Vec<String>> {
    let uri = url::Url::from_file_path(file)
        .map_err(|_| anyhow::anyhow!("invalid path {}", file.display()))?
        .to_string();
    let symbols = crate::tools::execute_lsp_query(
        remote,
        root,
        file,
        "textDocument/documentSymbol",
        serde_json::json!({ "textDocument": { "uri": uri } }),
    )
    .await?;
    let mut out = Vec::new();
    let mut level = symbols.as_array().cloned().unwrap_or_default();
    loop {
        let inside = |s: &serde_json::Value| {
            let at = |p: &str| s.pointer(p).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            at("/range/start/line") <= line && line <= at("/range/end/line")
        };
        let Some(module) = level
            .iter()
            .find(|s| s.get("kind").and_then(|k| k.as_u64()) == Some(2) && inside(s))
            .cloned()
        else {
            break;
        };
        out.push(
            module
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default()
                .to_string(),
        );
        level = module
            .get("children")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
    }
    Ok(out)
}
