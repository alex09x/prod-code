/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{ManifestOverlay, join_checked};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use toml::Value;

/// Cargo's explicit paths and conventional targets. Target aliases are allowed.
pub(super) fn roots(
    overlay: &ManifestOverlay<'_>,
    dir: &Path,
    doc: &Value,
) -> BTreeMap<PathBuf, bool> {
    let mut out = BTreeMap::new();
    let package = &doc["package"];
    let mut add = |raw: &str, proven: bool| {
        if let Some(path) = join_checked(dir, Path::new(raw))
            && overlay.files.contains(&path)
        {
            *out.entry(path).or_insert(false) |= proven;
        }
    };
    if let Some(lib) = doc.get("lib") {
        add(
            lib.get("path")
                .and_then(Value::as_str)
                .unwrap_or("src/lib.rs"),
            true,
        );
    } else if package.get("autolib").and_then(Value::as_bool) != Some(false) {
        add("src/lib.rs", true);
    }
    match package.get("build") {
        Some(Value::Boolean(false)) => {}
        Some(Value::String(path)) => add(path, true),
        _ => add("build.rs", true),
    }
    for (section, folder, auto) in [
        ("bin", "src/bin", "autobins"),
        ("example", "examples", "autoexamples"),
        ("test", "tests", "autotests"),
        ("bench", "benches", "autobenches"),
    ] {
        let mut explicit_names = BTreeSet::new();
        if let Some(targets) = doc.get(section).and_then(Value::as_array) {
            for target in targets {
                let proven = target
                    .get("required-features")
                    .and_then(Value::as_array)
                    .is_none_or(Vec::is_empty);
                if let Some(name) = target.get("name").and_then(Value::as_str) {
                    explicit_names.insert(name.to_string());
                }
                if let Some(path) = target.get("path").and_then(Value::as_str) {
                    add(path, proven);
                } else if let Some(name) = target.get("name").and_then(Value::as_str) {
                    add(&format!("{folder}/{name}.rs"), proven);
                    add(&format!("{folder}/{name}/main.rs"), proven);
                    if section == "bin" && package.get("name").and_then(Value::as_str) == Some(name)
                    {
                        add("src/main.rs", proven);
                    }
                }
            }
        }
        if package.get(auto).and_then(Value::as_bool) == Some(false) {
            continue;
        }
        if section == "bin"
            && !explicit_names.contains(package.get("name").and_then(Value::as_str).unwrap_or(""))
        {
            add("src/main.rs", true);
        }
        let base = dir.join(folder);
        for file in &overlay.files {
            let Ok(rel) = file.strip_prefix(&base) else {
                continue;
            };
            let n = rel.components().count();
            let name = if n == 1 {
                rel.file_stem()
            } else {
                rel.parent().and_then(Path::file_name)
            };
            if name.is_some_and(|n| explicit_names.contains(n.to_string_lossy().as_ref())) {
                continue;
            }
            if ((n == 1 && rel.extension().is_some_and(|e| e == "rs"))
                || (n == 2 && rel.file_name().is_some_and(|n| n == "main.rs")))
                && let Ok(path) = file.strip_prefix(dir)
            {
                add(&path.to_string_lossy(), true);
            }
        }
    }
    out
}
