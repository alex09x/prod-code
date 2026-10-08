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

use super::options::OutlineOptions;
use super::render::render_outline_with;
use crate::tools::{SKIPPED_DIRS, source_files};

/// Outlines every source file directly in `dir_path` using a single [`crate::session::LspSession`],
/// file by file, skipping files the gateway cannot outline, and ends with how many files were
/// outlined and how many skipped.
pub async fn outline_directory(
    remote: SocketAddr,
    workspace_root: &Path,
    dir_path: &Path,
    path_display_prefix: &Path,
    options: &OutlineOptions,
) -> Result<String> {
    let read_dir = std::fs::read_dir(dir_path)
        .with_context(|| format!("Failed to read directory {:?}", dir_path))?;
    let mut entries = Vec::new();
    let mut skipped = 0usize;
    let mut tests_left_out = 0usize;
    for entry in read_dir.flatten() {
        if !entry.path().is_file() {
            continue;
        }
        // A package's exports are not its tests' (Go's `TestX` is capitalised all the same).
        if options.exported_only && is_test_file(&entry.file_name().to_string_lossy()) {
            tests_left_out += 1;
            continue;
        }
        // Only source files a language server outlines: a manifest or a README is not asked
        // for, since a server that was handed one could answer with a made-up outline (#247).
        if let Some(engine) = crate::sync::engine_for_file(&entry.path())
            && !matches!(
                engine,
                "markdown" | "yaml" | "toml" | "json" | "html" | "css"
            )
        {
            entries.push(entry);
        } else {
            skipped += 1;
        }
    }
    if entries.is_empty() {
        return Ok(subdirectory_listing(dir_path, path_display_prefix, skipped));
    }
    entries.sort_by_key(|e| e.file_name());
    let mut session =
        crate::session::LspSession::open(remote, workspace_root, Some(dir_path)).await?;

    let max_bytes = options.max_bytes.unwrap_or(usize::MAX);
    let max_items = options.max_items.unwrap_or(usize::MAX);
    let mut blocks: Vec<String> = Vec::new();
    let mut used = 0usize;
    let mut listed = 0usize;
    let mut outlined = 0usize;
    let mut not_reached: Vec<String> = Vec::new();
    let mut stopped = false;
    for entry in entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if stopped {
            not_reached.push(name);
            continue;
        }
        let entry_file = entry.path();
        let file_uri = match Url::from_file_path(&entry_file) {
            Ok(u) => u.to_string(),
            Err(_) => {
                skipped += 1;
                continue;
            }
        };
        let params = serde_json::json!({
            "textDocument": { "uri": file_uri }
        });
        match session
            .query(&entry_file, "textDocument/documentSymbol", params)
            .await
        {
            Ok(res) if res.is_array() => {
                let display_path = path_display_prefix.join(&name).display().to_string();
                let source = std::fs::read_to_string(&entry_file).ok();
                let (block, count) =
                    render_outline_with(&res, &display_path, options, source.as_deref());
                if used + block.len() + 2 <= max_bytes && listed + count <= max_items {
                    used += block.len() + 2;
                    listed += count;
                    outlined += 1;
                    blocks.push(block);
                    continue;
                }
                stopped = true;
                if blocks.is_empty() {
                    // The first file alone is over the budget: as much of it as fits.
                    let (partial, kept) = cut_block(&block, max_bytes, max_items);
                    blocks.push(partial);
                    listed += kept;
                    outlined += 1;
                } else {
                    not_reached.push(name);
                }
            }
            _ => {
                skipped += 1;
            }
        }
    }

    let mut summary =
        format!("{outlined} file(s) outlined, {skipped} skipped, {listed} symbol(s) listed");
    if tests_left_out > 0 {
        summary.push_str(&format!(", {tests_left_out} test file(s) left out"));
    }
    if stopped {
        let limit = if listed >= max_items {
            format!("the limit of {max_items} symbols")
        } else {
            format!("the budget of {max_bytes} bytes")
        };
        summary.push_str(&format!("\nThe listing stops at {limit}"));
        if not_reached.is_empty() {
            summary.push('.');
        } else {
            let shown: Vec<&str> = not_reached.iter().take(20).map(String::as_str).collect();
            summary.push_str(&format!(
                "; {} more file(s) not outlined: {}{}.",
                not_reached.len(),
                shown.join(", "),
                if not_reached.len() > shown.len() {
                    format!(" and {} more", not_reached.len() - shown.len())
                } else {
                    String::new()
                }
            ));
        }
        summary.push_str(
            " Narrow it with `kinds` or `exported_only`, raise `max_bytes`, or outline one file.",
        );
    }
    Ok(format!("{}\n\n{summary}", blocks.join("\n\n")))
}

/// Whether a file name is a test file by its language's convention: `_test.go`, `*.test.ts`,
/// `*.spec.js`, `test_*.py`, `*_test.py`.
fn is_test_file(name: &str) -> bool {
    let (stem, extension) = name.rsplit_once('.').unwrap_or((name, ""));
    match extension {
        "go" => stem.ends_with("_test"),
        "py" => stem.starts_with("test_") || stem.ends_with("_test"),
        "ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs" => {
            stem.ends_with(".test") || stem.ends_with(".spec")
        }
        _ => false,
    }
}

/// For a directory with no source files of its own (a Go module's `internal/`), its
/// subdirectories that have some, with how many: where an outline finds something (#368).
fn subdirectory_listing(dir: &Path, display: &Path, skipped: usize) -> String {
    let mut subdirs: Vec<(String, usize)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || SKIPPED_DIRS.contains(&name.as_str()) {
                return None;
            }
            let sources = source_files(&entry.path())
                .filter(|path| crate::sync::engine_for_file(path).is_some())
                .take(10_000)
                .count();
            (sources > 0).then_some((name, sources))
        })
        .collect();
    subdirs.sort();
    let shown = display.display();
    if subdirs.is_empty() {
        return format!("0 file(s) outlined, {skipped} skipped: {shown} has no source files");
    }
    let mut out = format!(
        "{shown} has no source files of its own; outline one of its {} subdirectories with sources:",
        subdirs.len()
    );
    for (name, sources) in subdirs {
        out.push_str(&format!(
            "\n  {} ({sources} source file(s))",
            display.join(&name).display()
        ));
    }
    out
}

/// As much of one outline block as fits `max_bytes` and `max_items`, with a line saying how
/// many symbols were left out; and how many it keeps.
pub(crate) fn cut_block(block: &str, max_bytes: usize, max_items: usize) -> (String, usize) {
    let symbols = block
        .lines()
        .filter(|l| l.trim_start().starts_with('['))
        .count();
    if block.len() <= max_bytes && symbols <= max_items {
        return (block.to_string(), symbols);
    }
    let mut lines = block.lines();
    let mut out = lines.next().unwrap_or_default().to_string();
    let mut kept = 0usize;
    for line in lines.filter(|l| l.trim_start().starts_with('[')) {
        if kept >= max_items || out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push('\n');
        out.push_str(line);
        kept += 1;
    }
    out.push_str(&format!(
        "\n  … {} more symbol(s) of this file not listed",
        symbols - kept
    ));
    (out, kept)
}
