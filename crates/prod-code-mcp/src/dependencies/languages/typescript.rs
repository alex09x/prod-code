/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

fn extract_quoted_string(line: &str) -> Option<&str> {
    let start = line.find('"')? + 1;
    let end = line[start..].find('"')? + start;
    Some(&line[start..end])
}

pub fn parse_ts_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if (trimmed.starts_with("import ") || trimmed.starts_with("export "))
            && trimmed.contains("from ")
            && let Some(path) = extract_quoted_string(trimmed)
        {
            // Only local relative imports: ./ or ../
            if path.starts_with('.') {
                let base = path.split('/').next_back().unwrap_or(path);
                for m in known_modules.keys() {
                    if m.ends_with(base) {
                        deps.insert(m.clone());
                    }
                }
            }
        }
    }
}
