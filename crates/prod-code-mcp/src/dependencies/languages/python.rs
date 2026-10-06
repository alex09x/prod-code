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

pub fn parse_python_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("import ") {
            let mod_part = trimmed
                .trim_start_matches("import ")
                .split([' ', ','])
                .next()
                .unwrap_or("");
            for m in known_modules.keys() {
                if m.ends_with(mod_part) {
                    deps.insert(m.clone());
                }
            }
        } else if trimmed.starts_with("from ") {
            let mod_part = trimmed
                .trim_start_matches("from ")
                .split(' ')
                .next()
                .unwrap_or("");
            let clean = mod_part.trim_start_matches('.');
            for m in known_modules.keys() {
                if m.ends_with(clean) {
                    deps.insert(m.clone());
                }
            }
        }
    }
}
