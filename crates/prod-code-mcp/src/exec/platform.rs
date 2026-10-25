/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;

// Retain the original public module path as well as the crate-level export.
pub use super::layout::layout_only;

/// What in a file makes its meaning depend on the platform: conditional compilation on the
/// target, and the C library, whose signatures differ between Linux and Apple (#140).
const PLATFORM_MARKS: &[&str] = &[
    "libc::",
    "#ifdef __APPLE__",
    "#if defined(__APPLE__)",
    "#ifdef __linux__",
    "#if os(",
    "//go:build ",
];

pub(super) fn platform_dependent(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes);
    if PLATFORM_MARKS.iter().any(|mark| text.contains(mark)) {
        return true;
    }
    // Conservatively recognize cfg/cfg_attr attributes and cfg! expressions, including
    // nested not/any/all predicates. False positives warn; missing evidence would be silent.
    static CFG: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\bcfg(?:_attr)?\s*!?\s*\(").expect("constant cfg regex")
    });
    CFG.is_match(&text)
        && (text.contains("target_") || text.contains("unix") || text.contains("windows"))
}

/// A warning for files a command rewrote on a node of another OS than this machine's, when
/// they hold code whose meaning depends on the platform, or the checkout is an Apple project: a
/// lint or a fix computed there can be wrong here, and it was written back as a success.
/// `None` when the platforms match, nothing was written, or nothing looks platform-specific.
pub fn platform_warning(root: &Path, node: Option<&str>, pulled: &[String]) -> Option<String> {
    platform_warning_with_evidence(root, node, pulled, &[])
}

pub(super) fn platform_warning_with_evidence(
    root: &Path,
    node: Option<&str>,
    pulled: &[String],
    before: &[String],
) -> Option<String> {
    let node = node?;
    let here = prod_code_protocol::platform();
    let os = |p: &str| p.split(' ').next().unwrap_or("").to_string();
    if pulled.is_empty() || os(node) == os(&here) {
        return None;
    }
    let marked: Vec<&String> = pulled
        .iter()
        .filter(|rel| {
            before.contains(rel)
                || std::fs::read(root.join(rel)).is_ok_and(|text| platform_dependent(&text))
        })
        .collect();
    let apple = root.join("Package.swift").is_file()
        || std::fs::read_dir(root).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x == "xcodeproj" || x == "xcworkspace")
            })
        });
    if marked.is_empty() && !apple {
        return None;
    }
    let what = if marked.is_empty() {
        "this checkout is also an Apple project".to_string()
    } else {
        format!(
            "{} of them hold code that depends on the platform ({})",
            marked.len(),
            marked
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    Some(format!(
        "warning: the command ran on {node} and this machine is {here}; {what}. A fix or a lint computed there can be wrong here (a `cfg` it never compiled, a libc signature that differs), so build the checkout here before trusting it."
    ))
}

/// The `/`-separated path of `dir` inside `root`, or `None` when `dir` is the root itself
/// or lies outside it: the directory a command runs in on the server.
pub fn subdir_of(root: &Path, dir: &Path) -> Option<String> {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let rel = dir.strip_prefix(&root).ok()?;
    let rel = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!rel.is_empty()).then_some(rel)
}
