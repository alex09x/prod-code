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
use std::path::Path;

/// Every path under `root` with its bytes (none for a directory) and permission bits: what a
/// failed edit has to leave exactly as it found it.
pub fn tree(root: &Path) -> BTreeMap<String, (Option<Vec<u8>>, u32)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            #[cfg(unix)]
            let mode = std::os::unix::fs::PermissionsExt::mode(&meta.permissions());
            #[cfg(not(unix))]
            let mode = 0;
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if meta.is_dir() {
                stack.push(path);
                out.insert(rel, (None, mode));
            } else {
                out.insert(rel, (Some(std::fs::read(&path).unwrap_or_default()), mode));
            }
        }
    }
    out
}

pub fn whole(text: &str) -> serde_json::Value {
    serde_json::json!([{ "range": { "start": { "line": 0, "character": 0 },
                                     "end": { "line": 1, "character": 0 } },
                          "newText": text }])
}

pub fn at(line: u64, start: u64, end: u64, text: &str) -> serde_json::Value {
    serde_json::json!([{ "range": { "start": { "line": line, "character": start },
                                     "end": { "line": line, "character": end } },
                          "newText": text }])
}
