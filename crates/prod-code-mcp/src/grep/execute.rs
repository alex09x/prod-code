/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Execution engine for `code_grep` / `prod-code grep`: remote `rg --json` over the mirrored
//! workspace with local `rg --json` and gitignore-aware `ignore::WalkBuilder` + `OverrideBuilder`
//! fallback, plus per-hit enclosing symbol and context-line enrichment (#1119).

use super::fallback::{enrich_matches, run_in_process_grep};
use super::{GrepOptions, GrepReport, MAX_CONTEXT_LINES, MAX_RESULTS_CAP};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

#[cfg(test)]
#[path = "execute_tests.rs"]
mod tests;

pub(super) const DEFAULT_EXCLUDES: &[&str] = &[
    "!target/**",
    "!node_modules/**",
    "!.git/**",
    "!dist/**",
    "!.build/**",
];

/// Raw match location before file-line context and enclosing-symbol enrichment.
#[derive(Debug, Clone)]
pub(super) struct RawMatch {
    pub(super) full_path: PathBuf,
    pub(super) rel_path: String,
    pub(super) line: u32,
    pub(super) column: u32,
    pub(super) line_text: Option<String>,
}

/// Executes a literal or regex search across `root` (or `opts.path`), preferring remote `rg --json`
/// on the gateway node when `remote` is available and falling back to local `rg --json` or
/// in-process `ignore::WalkBuilder`.
pub async fn run_grep(
    root: &Path,
    remote: Option<SocketAddr>,
    opts: &GrepOptions,
) -> Result<GrepReport> {
    if opts.pattern.is_empty() {
        bail!("code_grep requires a non-empty `pattern`");
    }
    let max_results = if opts.max_results == 0 {
        super::DEFAULT_MAX_RESULTS
    } else {
        opts.max_results.clamp(1, MAX_RESULTS_CAP)
    };
    let context = opts.context.min(MAX_CONTEXT_LINES);

    let search_path = match &opts.path {
        Some(p) if p.is_absolute() => p.clone(),
        Some(p) => root.join(p),
        None => root.to_path_buf(),
    };
    if !search_path.exists() {
        let hint = crate::remote_fs::suggest_nearest_path(root, &search_path)
            .map(|h| format!(" (did you mean `{h}`?)"))
            .unwrap_or_default();
        bail!(
            "search path does not exist: {}{hint}",
            search_path.display()
        );
    }

    let root = std::fs::canonicalize(root).context("workspace root cannot be resolved")?;
    let search_path = std::fs::canonicalize(&search_path)
        .with_context(|| format!("search path cannot be resolved: {}", search_path.display()))?;
    if !search_path.starts_with(&root) {
        bail!(
            "search path is outside the workspace: {}",
            search_path.display()
        );
    }

    let mut attempted_local = false;
    if search_path.is_file() {
        attempted_local = true;
        let root_owned = root.to_path_buf();
        let path_owned = search_path.clone();
        let opts_owned = opts.clone();
        if let Some(report) = tokio::task::spawn_blocking(move || {
            try_local_rg(&root_owned, &path_owned, &opts_owned, max_results, context)
        })
        .await
        .context("grep worker task panicked")??
        {
            return Ok(report);
        }
    }

    if let Some(addr) = remote
        && search_path.starts_with(&root)
        && let Ok(Some(report)) =
            try_remote_rg(addr, &root, &search_path, opts, max_results, context).await
    {
        return Ok(report);
    }

    let root_owned = root.to_path_buf();
    let opts_owned = opts.clone();
    tokio::task::spawn_blocking(move || {
        if !attempted_local
            && let Ok(Some(report)) =
                try_local_rg(&root_owned, &search_path, &opts_owned, max_results, context)
        {
            return Ok(report);
        }
        run_in_process_grep(&root_owned, &search_path, &opts_owned, max_results, context)
    })
    .await
    .context("grep worker task panicked")?
}

fn build_rg_args(rel_target: &str, opts: &GrepOptions) -> Vec<String> {
    let mut cmd = vec![
        "--json".to_string(),
        "--line-number".to_string(),
        "--column".to_string(),
        "--no-heading".to_string(),
        "--color=never".to_string(),
    ];
    if opts.fixed_strings {
        cmd.push("--fixed-strings".to_string());
    }
    if opts.ignore_case {
        cmd.push("--ignore-case".to_string());
    } else {
        cmd.push("--smart-case".to_string());
    }
    for exc in DEFAULT_EXCLUDES {
        cmd.push("-g".to_string());
        cmd.push((*exc).to_string());
    }
    for inc in &opts.include {
        let trimmed = inc.trim();
        if !trimmed.is_empty() {
            cmd.push("-g".to_string());
            cmd.push(trimmed.to_string());
        }
    }
    for exc in &opts.exclude {
        let trimmed = exc.trim().trim_start_matches('!');
        if !trimmed.is_empty() {
            cmd.push("-g".to_string());
            cmd.push(format!("!{trimmed}"));
        }
    }
    cmd.push("-e".to_string());
    cmd.push(opts.pattern.clone());
    cmd.push(rel_target.to_string());
    cmd
}

async fn try_remote_rg(
    addr: SocketAddr,
    root: &Path,
    search_path: &Path,
    opts: &GrepOptions,
    max_results: usize,
    context: usize,
) -> Result<Option<GrepReport>> {
    let rel_target = search_path
        .strip_prefix(root)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| ".".to_string());

    let mut cmd = vec!["rg".to_string()];
    cmd.extend(build_rg_args(&rel_target, opts));

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let outcome = crate::exec::run_remote(
        addr,
        root,
        None,
        cmd,
        Vec::new(),
        30,
        false,
        |is_err, chunk| {
            if is_err {
                stderr.extend_from_slice(chunk);
            } else {
                stdout.extend_from_slice(chunk);
            }
        },
    )
    .await?;

    if outcome.exit.error.is_some() || outcome.exit.timed_out {
        return Ok(None);
    }
    match outcome.exit.exit_code {
        Some(0 | 1) => {}
        Some(2) => {
            let err_str = String::from_utf8_lossy(&stderr);
            if err_str.contains("regex parse error") {
                bail!(
                    "invalid regex pattern `{}`: {}",
                    opts.pattern,
                    err_str.trim()
                );
            }
            return Ok(None);
        }
        _ => return Ok(None),
    }

    Ok(Some(parse_rg_json_output(
        root,
        &stdout,
        opts,
        max_results,
        context,
        "remote-ripgrep",
    )))
}

fn try_local_rg(
    root: &Path,
    search_path: &Path,
    opts: &GrepOptions,
    max_results: usize,
    context: usize,
) -> Result<Option<GrepReport>> {
    let rel_target = search_path
        .strip_prefix(root)
        .ok()
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|| search_path.to_string_lossy().into_owned());

    let args = build_rg_args(&rel_target, opts);
    let output = match std::process::Command::new("rg")
        .args(&args)
        .current_dir(root)
        .output()
    {
        Ok(o) => o,
        Err(_) => return Ok(None),
    };

    match output.status.code() {
        Some(0 | 1) => {}
        Some(2) => {
            let err_str = String::from_utf8_lossy(&output.stderr);
            if err_str.contains("regex parse error") {
                bail!(
                    "invalid regex pattern `{}`: {}",
                    opts.pattern,
                    err_str.trim()
                );
            }
            return Ok(None);
        }
        _ => return Ok(None),
    }

    Ok(Some(parse_rg_json_output(
        root,
        &output.stdout,
        opts,
        max_results,
        context,
        "local-ripgrep",
    )))
}

fn parse_rg_json_output(
    root: &Path,
    stdout: &[u8],
    opts: &GrepOptions,
    max_results: usize,
    context: usize,
    engine: &str,
) -> GrepReport {
    let mut raw_matches = Vec::new();
    let mut total_matches = 0usize;
    let mut truncated = false;
    for line in String::from_utf8_lossy(stdout).lines() {
        let Ok(ev) = serde_json::from_str::<RgJsonEvent>(line) else {
            continue;
        };
        if ev.kind != "match" {
            continue;
        }
        let Some(data) = ev.data else {
            continue;
        };
        total_matches += 1;
        if raw_matches.len() >= max_results {
            truncated = true;
            continue;
        }
        let raw_p = PathBuf::from(&data.path.text);
        let full_path = if raw_p.is_absolute() {
            raw_p.clone()
        } else {
            root.join(data.path.text.trim_start_matches("./"))
        };
        let rel = full_path
            .strip_prefix(root)
            .unwrap_or(&full_path)
            .to_string_lossy()
            .replace('\\', "/");
        let col = data
            .submatches
            .first()
            .map(|s| (s.start as u32) + 1)
            .unwrap_or(1);
        raw_matches.push(RawMatch {
            full_path,
            rel_path: rel,
            line: data.line_number,
            column: col,
            line_text: data
                .lines
                .text
                .map(|t| t.trim_end_matches(['\r', '\n']).to_string()),
        });
    }

    let hits = enrich_matches(&raw_matches, context);
    GrepReport {
        pattern: opts.pattern.clone(),
        hits,
        total_matches,
        truncated,
        engine: engine.to_string(),
    }
}

#[derive(Deserialize)]
struct RgJsonEvent {
    #[serde(rename = "type")]
    kind: String,
    data: Option<RgMatchData>,
}

#[derive(Deserialize)]
struct RgMatchData {
    path: RgTextValue,
    lines: RgOptionalTextValue,
    line_number: u32,
    #[serde(default)]
    submatches: Vec<RgSubmatch>,
}

#[derive(Deserialize)]
struct RgTextValue {
    text: String,
}

#[derive(Deserialize)]
struct RgOptionalTextValue {
    text: Option<String>,
}

#[derive(Deserialize)]
struct RgSubmatch {
    start: usize,
}
