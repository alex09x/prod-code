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

pub fn parse_csharp_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("using ") && trimmed.ends_with(';') {
            let rest = trimmed
                .trim_start_matches("using ")
                .trim_start_matches("static ")
                .trim_start_matches("unsafe ")
                .trim_end_matches(';')
                .trim();
            let target = if let Some(idx) = rest.find('=') {
                rest[idx + 1..].trim()
            } else {
                rest
            };
            let as_colons = target.replace('.', "::");
            let type_name = target.rsplit('.').next().unwrap_or("");
            for mod_name in known_modules.keys() {
                if mod_name.ends_with(&as_colons)
                    || (!type_name.is_empty() && mod_name.ends_with(&format!("::{type_name}")))
                {
                    deps.insert(mod_name.clone());
                }
            }
        }
    }
}
