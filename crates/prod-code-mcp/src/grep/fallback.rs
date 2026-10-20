/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! In-process fallback walker (`ignore::WalkBuilder` + `OverrideBuilder`) and per-hit
//! context/enclosing-symbol enrichment for `code_grep` (#1119).

use super::execute::{DEFAULT_EXCLUDES, RawMatch};
use super::{GrepHit, GrepOptions, GrepReport, find_enclosing_symbol_in_lines};
use anyhow::{Context, Result};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn run_in_process_grep(
    root: &Path,
    search_path: &Path,
    opts: &GrepOptions,
    max_results: usize,
    context: usize,
) -> Result<GrepReport> {
    let case_insensitive = opts.ignore_case || !opts.pattern.chars().any(char::is_uppercase);
    let branches: Vec<String> = if opts.fixed_strings {
        vec![opts.pattern.clone()]
    } else {
        opts.pattern
            .split('|')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    };

    let mut overrides = OverrideBuilder::new(search_path);
    for exc in DEFAULT_EXCLUDES {
        let _ = overrides.add(exc);
    }
    for inc in &opts.include {
        let trimmed = inc.trim();
        if !trimmed.is_empty() {
            overrides
                .add(trimmed)
                .with_context(|| format!("invalid include glob `{trimmed}`"))?;
        }
    }
    for exc in &opts.exclude {
        let trimmed = exc.trim().trim_start_matches('!');
        if !trimmed.is_empty() {
            overrides
                .add(&format!("!{trimmed}"))
                .with_context(|| format!("invalid exclude glob `{trimmed}`"))?;
        }
    }
    let built_overrides = overrides
        .build()
        .context("failed to build glob overrides")?;

    let mut walker = WalkBuilder::new(search_path);
    walker
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .overrides(built_overrides)
        .sort_by_file_path(|a, b| a.cmp(b));

    let mut raw_matches = Vec::new();
    let mut total_matches = 0usize;
    let mut truncated = false;

    for entry in walker.build().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(path) else {
            continue;
        };
        if bytes.iter().take(8000).any(|&b| b == 0) {
            continue;
        }
        let Ok(content) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let rel_path = path
            .strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        for (idx, line_str) in content.lines().enumerate() {
            if let Some(col0) = match_line(line_str, &branches, case_insensitive) {
                total_matches += 1;
                if raw_matches.len() < max_results {
                    raw_matches.push(RawMatch {
                        full_path: path.to_path_buf(),
                        rel_path: rel_path.clone(),
                        line: (idx + 1) as u32,
                        column: (col0 + 1) as u32,
                        line_text: Some(line_str.to_string()),
                    });
                } else {
                    truncated = true;
                }
            }
        }
    }

    let hits = enrich_matches(&raw_matches, context);
    Ok(GrepReport {
        pattern: opts.pattern.clone(),
        hits,
        total_matches,
        truncated,
        engine: "ignore-walk".to_string(),
    })
}

fn match_line(line: &str, branches: &[String], case_insensitive: bool) -> Option<usize> {
    if case_insensitive {
        let lower_line = line.to_ascii_lowercase();
        branches
            .iter()
            .filter_map(|b| lower_line.find(&b.to_ascii_lowercase()))
            .min()
    } else {
        branches.iter().filter_map(|b| line.find(b.as_str())).min()
    }
}

pub(super) fn enrich_matches(raw_matches: &[RawMatch], context: usize) -> Vec<GrepHit> {
    let mut file_cache: HashMap<PathBuf, Vec<String>> = HashMap::new();
    let mut hits = Vec::with_capacity(raw_matches.len());

    for raw in raw_matches {
        let lines = file_cache.entry(raw.full_path.clone()).or_insert_with(|| {
            std::fs::read_to_string(&raw.full_path)
                .map(|s| s.lines().map(str::to_string).collect())
                .unwrap_or_default()
        });
        let line_idx = raw.line.saturating_sub(1) as usize;
        let text = raw
            .line_text
            .clone()
            .or_else(|| lines.get(line_idx).cloned())
            .unwrap_or_default();
        let enclosing_symbol = if lines.is_empty() {
            None
        } else {
            find_enclosing_symbol_in_lines(lines, raw.line)
        };

        let mut context_before = Vec::new();
        let mut context_after = Vec::new();
        if context > 0 && !lines.is_empty() {
            let start = line_idx.saturating_sub(context);
            for (i, l) in lines.iter().enumerate().take(line_idx).skip(start) {
                context_before.push(((i + 1) as u32, l.clone()));
            }
            let end = (line_idx + 1 + context).min(lines.len());
            for (i, l) in lines.iter().enumerate().take(end).skip(line_idx + 1) {
                context_after.push(((i + 1) as u32, l.clone()));
            }
        }

        hits.push(GrepHit {
            path: raw.rel_path.clone(),
            line: raw.line,
            column: raw.column,
            text,
            enclosing_symbol,
            context_before,
            context_after,
        });
    }
    hits
}
