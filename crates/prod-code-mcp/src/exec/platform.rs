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

/// Whether `new` has the same characters as `old` apart from whitespace, commas and braces, in
/// any order, and is not the same text: what a formatter makes of a file. rustfmt re-wraps
/// lines, adds trailing commas, sorts imports and drops the braces around a closure whose body
/// is one expression (#244). A real edit, such as `&mut win` to `&win`, adds or removes other
/// characters.
pub fn layout_only(old: &[u8], new: &[u8]) -> bool {
    let counts = |text: &[u8]| {
        let mut counts = [0usize; 256];
        for &b in text {
            if !b.is_ascii_whitespace() && !matches!(b, b',' | b'{' | b'}') {
                counts[b as usize] += 1;
            }
        }
        counts
    };
    old != new && counts(old) == counts(new)
}

/// What in a file makes its meaning depend on the platform: conditional compilation on the
/// target, and the C library, whose signatures differ between Linux and Apple (#140).
const PLATFORM_MARKS: &[&str] = &[
    "cfg(target_",
    "cfg(unix)",
    "cfg(windows)",
    "libc::",
    "#ifdef __APPLE__",
    "#if defined(__APPLE__)",
    "#ifdef __linux__",
    "#if os(",
    "//go:build ",
];

/// A warning for files a command rewrote on a node of another OS than this machine's, when
/// they hold code whose meaning depends on the platform, or the checkout is an Apple project: a
/// lint or a fix computed there can be wrong here, and it was written back as a success.
/// `None` when the platforms match, nothing was written, or nothing looks platform-specific.
pub fn platform_warning(root: &Path, node: Option<&str>, pulled: &[String]) -> Option<String> {
    let node = node?;
    let here = prod_code_protocol::platform();
    let os = |p: &str| p.split(' ').next().unwrap_or("").to_string();
    if pulled.is_empty() || os(node) == os(&here) {
        return None;
    }
    let marked: Vec<&String> = pulled
        .iter()
        .filter(|rel| {
            std::fs::read_to_string(root.join(rel))
                .is_ok_and(|text| PLATFORM_MARKS.iter().any(|m| text.contains(m)))
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

/// Whether `path` was modified after `since`, on this machine's clock. A file that does not
/// exist was not.
pub(crate) fn edited_since(path: &Path, since: std::time::SystemTime) -> bool {
    std::fs::symlink_metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|modified| modified > since)
}
