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

pub fn parse_rust_imports(
    content: &str,
    known_modules: &HashMap<String, PathBuf>,
    deps: &mut BTreeSet<String>,
) {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("use crate::") || trimmed.starts_with("use super::") {
            let path_part = trimmed
                .trim_start_matches("use crate::")
                .trim_start_matches("use super::")
                .split([';', ':', ' ', '{'])
                .next()
                .unwrap_or("");
            if !path_part.is_empty() {
                for mod_name in known_modules.keys() {
                    if mod_name.ends_with(path_part) || mod_name.contains(&format!("::{path_part}"))
                    {
                        deps.insert(mod_name.clone());
                    }
                }
            }
        } else if trimmed.starts_with("mod ") && trimmed.ends_with(';') {
            let mod_name = trimmed
                .trim_start_matches("mod ")
                .trim_end_matches(';')
                .trim();
            for m in known_modules.keys() {
                if m.ends_with(&format!("::{mod_name}"))
                    || m.ends_with(&format!("::{mod_name}::mod"))
                {
                    deps.insert(m.clone());
                }
            }
        }
    }
}
