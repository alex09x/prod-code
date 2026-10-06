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

pub fn parse_java_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            let rest = trimmed
                .trim_start_matches("import ")
                .trim_start_matches("static ")
                .trim_end_matches(';')
                .trim();
            let as_colons = rest.replace('.', "::");
            let class_name = rest.rsplit('.').next().unwrap_or("");
            for mod_name in known_modules.keys() {
                if mod_name.ends_with(&as_colons)
                    || (!class_name.is_empty() && mod_name.ends_with(&format!("::{class_name}")))
                {
                    deps.insert(mod_name.clone());
                }
            }
        }
    }
}
