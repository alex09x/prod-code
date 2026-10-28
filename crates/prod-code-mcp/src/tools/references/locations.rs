/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use url::Url;

use crate::tools::symbols::is_in_nested_git_worktree;

pub(crate) fn add_locations(
    workspace_root: &Path,
    locations: &serde_json::Value,
    seen_locs: &mut HashSet<(String, u64, u64)>,
    seen_files: &mut HashSet<PathBuf>,
    all_locs: &mut Vec<serde_json::Value>,
) {
    if let Some(arr) = locations.as_array() {
        for loc in arr {
            let uri = loc
                .get("uri")
                .or_else(|| loc.get("targetUri"))
                .and_then(|u| u.as_str())
                .unwrap_or("");
            let parsed_path = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok());
            if let Some(ref path) = parsed_path
                && is_in_nested_git_worktree(workspace_root, path)
            {
                continue;
            }
            let start_line = loc
                .pointer("/range/start/line")
                .or_else(|| loc.pointer("/targetRange/start/line"))
                .and_then(|l| l.as_u64())
                .unwrap_or(0)
                + 1;
            let start_col = loc
                .pointer("/range/start/character")
                .or_else(|| loc.pointer("/targetRange/start/character"))
                .and_then(|c| c.as_u64())
                .unwrap_or(0)
                + 1;
            if seen_locs.insert((uri.to_string(), start_line, start_col)) {
                all_locs.push(loc.clone());
                if let Some(path) = parsed_path {
                    let norm = std::fs::canonicalize(&path).unwrap_or(path);
                    seen_files.insert(norm);
                }
            }
        }
    }
}

pub(super) fn record_file_uses(
    uri_str: &str,
    uses: &[(usize, usize)],
    // The name's length in UTF-16 code units, as the columns of `uses` are counted.
    units: usize,
    seen_locs: &mut HashSet<(String, u64, u64)>,
    all_locs: &mut Vec<serde_json::Value>,
) {
    for &(u_idx, u_col) in uses {
        let use_line = (u_idx + 1) as u64;
        let use_col = (u_col + 1) as u64;
        if seen_locs.insert((uri_str.to_string(), use_line, use_col)) {
            all_locs.push(serde_json::json!({
                "uri": uri_str,
                "range": {
                    "start": { "line": u_idx, "character": u_col },
                    "end": { "line": u_idx, "character": u_col + units }
                }
            }));
        }
    }
}
