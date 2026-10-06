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

pub fn parse_go_imports(
    content: &str,
    go_module_prefix: Option<&str>,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    let mut in_import_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import (") {
            in_import_block = true;
            continue;
        }
        if in_import_block && trimmed == ")" {
            in_import_block = false;
            continue;
        }

        if (in_import_block || trimmed.starts_with("import "))
            && let Some(pkg) = extract_quoted_string(trimmed)
        {
            // Only resolve internal workspace packages
            let target_pkg = if let Some(prefix) = go_module_prefix {
                if pkg == prefix {
                    Some("")
                } else {
                    pkg.strip_prefix(&format!("{prefix}/"))
                }
            } else if let Some(rel) = pkg.strip_prefix("./") {
                Some(rel)
            } else {
                pkg.strip_prefix("../")
            };

            if let Some(target_dir) = target_pkg {
                let target_colon = target_dir.replace('/', "::");
                for mod_name in known_modules.keys() {
                    if target_colon.is_empty() {
                        if !mod_name.contains("::") {
                            deps.insert(mod_name.clone());
                        }
                    } else if mod_name.starts_with(&format!("{target_colon}::")) {
                        deps.insert(mod_name.clone());
                    }
                }
            }
        }
    }
}
