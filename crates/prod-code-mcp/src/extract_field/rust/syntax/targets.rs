/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::fixture::builder::lexer::Source;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// A Cargo package can build multiple distinct crates. Only narrow to pub(crate)
/// when both source files have the same unambiguous target membership. Unknown or
/// shared module layouts require public access for the new field alone.
pub(super) fn same_crate(a: &Path, b: &Path) -> bool {
    let Some(dir) = a.ancestors().find(|d| d.join("Cargo.toml").is_file()) else {
        return false;
    };
    if b.ancestors().find(|d| d.join("Cargo.toml").is_file()) != Some(dir) {
        return false;
    }
    let Some(roots) = roots(dir) else {
        return false;
    };
    let mut owners_a = BTreeSet::new();
    let mut owners_b = BTreeSet::new();
    for root in roots {
        let mut files = BTreeSet::new();
        if !modules(&root, root.parent().unwrap(), &mut files, 0) {
            return false;
        }
        if files.contains(a) {
            owners_a.insert(root.clone());
        }
        if files.contains(b) {
            owners_b.insert(root);
        }
    }
    owners_a.len() == 1 && owners_a == owners_b
}

fn roots(dir: &Path) -> Option<BTreeSet<PathBuf>> {
    let raw = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let value: toml::Value = toml::from_str(&raw).ok()?;
    let mut roots = BTreeSet::new();
    let lib = value
        .get("lib")
        .and_then(|v| v.get("path"))
        .and_then(|v| v.as_str())
        .unwrap_or("src/lib.rs");
    roots.insert(dir.join(lib));
    for (kind, folder, auto) in [
        ("bin", "src/bin", "autobins"),
        ("example", "examples", "autoexamples"),
        ("test", "tests", "autotests"),
        ("bench", "benches", "autobenches"),
    ] {
        if let Some(targets) = value.get(kind).and_then(|v| v.as_array()) {
            for target in targets {
                if let Some(path) = target.get("path").and_then(|v| v.as_str()) {
                    roots.insert(dir.join(path));
                }
            }
        }
        if value
            .get("package")
            .and_then(|p| p.get(auto))
            .and_then(|v| v.as_bool())
            == Some(false)
        {
            continue;
        }
        if kind == "bin" {
            roots.insert(dir.join("src/main.rs"));
        }
        if let Ok(entries) = std::fs::read_dir(dir.join(folder)) {
            for entry in entries.flatten() {
                let path = entry.path();
                roots.insert(if path.is_dir() {
                    path.join("main.rs")
                } else {
                    path
                });
            }
        }
    }
    roots.retain(|p| p.extension().is_some_and(|s| s == "rs") && p.is_file());
    Some(roots)
}

fn modules(file: &Path, dir: &Path, files: &mut BTreeSet<PathBuf>, depth: usize) -> bool {
    if depth > 16 || files.len() > 128 {
        return false;
    }
    if !files.insert(file.to_path_buf()) {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(s) = Source::new(&text) else {
        return false;
    };
    // Custom module paths and source inclusion need analyzer-backed crate identity.
    if s.tokens.iter().enumerate().any(|(i, _)| {
        (s.is(i, "path") && s.is(i + 1, "=")) || (s.is(i, "include") && s.is(i + 1, "!"))
    }) {
        return false;
    }
    walk(&s, 0, s.tokens.len(), dir, files, depth)
}

fn walk(
    s: &Source<'_>,
    from: usize,
    to: usize,
    dir: &Path,
    files: &mut BTreeSet<PathBuf>,
    depth: usize,
) -> bool {
    let mut i = from;
    while i + 2 < to {
        if s.is(i, "mod") {
            let name = s.t(i + 1).trim_start_matches("r#");
            if s.is(i + 2, "{") {
                let Some(end) = s.close_of(i + 2) else {
                    return false;
                };
                if !walk(s, i + 3, end, &dir.join(name), files, depth + 1) {
                    return false;
                }
                i = end + 1;
                continue;
            }
            if s.is(i + 2, ";") {
                let direct = dir.join(format!("{name}.rs"));
                let nested = dir.join(name).join("mod.rs");
                let child = if direct.is_file() { direct } else { nested };
                if child.is_file() && !modules(&child, &dir.join(name), files, depth + 1) {
                    return false;
                }
            }
        }
        i += 1;
    }
    true
}
