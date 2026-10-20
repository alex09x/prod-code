/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Reference location rendering with enclosing symbol annotations and optional ±N line context (#1122).

use std::collections::HashMap;
use std::path::Path;

use crate::tools::resolve_file_path;

pub(crate) fn render_reference_locations(
    workspace_root: &Path,
    locs: &[serde_json::Value],
    context_lines: usize,
) -> String {
    let mut out = String::new();
    let mut file_cache: HashMap<String, Option<Vec<String>>> = HashMap::new();

    for loc in locs {
        let uri = loc
            .get("uri")
            .or_else(|| loc.get("targetUri"))
            .and_then(|u| u.as_str())
            .unwrap_or("");
        let start_line = (loc
            .pointer("/range/start/line")
            .or_else(|| loc.pointer("/targetRange/start/line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1) as usize;
        let start_col = (loc
            .pointer("/range/start/character")
            .or_else(|| loc.pointer("/targetRange/start/character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1) as usize;

        let lines_opt = file_cache
            .entry(uri.to_string())
            .or_insert_with(|| load_uri_lines(workspace_root, uri))
            .as_ref();

        let enclosing = lines_opt.and_then(|lines| {
            crate::grep::find_enclosing_symbol_in_lines(lines, start_line as u32)
        });
        match enclosing {
            Some(sym) => out.push_str(&format!("  • {uri}:{start_line}:{start_col} [in {sym}]\n")),
            None => out.push_str(&format!("  • {uri}:{start_line}:{start_col}\n")),
        }

        if context_lines > 0
            && let Some(lines) = lines_opt
            && !lines.is_empty()
            && start_line >= 1
            && start_line <= lines.len()
        {
            let from = start_line.saturating_sub(context_lines).max(1);
            let to = (start_line + context_lines).min(lines.len());
            let width = to.to_string().len().max(3);
            for ln in from..=to {
                let marker = if ln == start_line { ">" } else { " " };
                let text = lines.get(ln - 1).map(String::as_str).unwrap_or("");
                out.push_str(&format!("    {marker} {ln:>width$} | {text}\n"));
            }
        }
    }

    out
}

fn load_uri_lines(workspace_root: &Path, uri: &str) -> Option<Vec<String>> {
    if uri.is_empty() {
        return None;
    }
    let raw_path = crate::remote_fs::uri_to_path(uri);
    let resolved = resolve_file_path(workspace_root, &raw_path);
    let text = std::fs::read_to_string(&resolved)
        .or_else(|_| std::fs::read_to_string(&raw_path))
        .ok()?;
    Some(text.lines().map(str::to_string).collect())
}
