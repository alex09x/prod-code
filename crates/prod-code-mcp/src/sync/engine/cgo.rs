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

/// Headers only Apple systems ship. A cgo preamble that includes one does not compile on a
/// Linux build node.
const APPLE_ONLY_HEADERS: &[&str] = &[
    "libproc.h",
    "mach/",
    "CoreFoundation/",
    "IOKit/",
    "Security/",
    "AppKit/",
    "Cocoa/",
];

/// How many `.go` files [`macos_only_cgo`] reads. It runs at every client start, so a larger
/// module is judged by the files seen by then.
const MACOS_CGO_SCAN_LIMIT: usize = 4000;

/// Maximum number of directories [`macos_only_cgo`] traverses to avoid unbounded filesystem walks.
const MACOS_CGO_DIR_SCAN_LIMIT: usize = 1000;

/// Directories that should never be searched for Go module source files.
fn is_ignored_dir(name: &str) -> bool {
    name.starts_with(['.', '_'])
        || matches!(
            name,
            "vendor"
                | "testdata"
                | "target"
                | "node_modules"
                | "build"
                | "dist"
                | "out"
                | "DerivedData"
        )
}

/// Why the Go project at `root` builds only on macOS: the first `.go` file (relative path) whose
/// cgo preamble includes a macOS-only header or links a framework, and what it names. A file that
/// Linux skips anyway, by a `//go:build` line Linux does not satisfy or a `_darwin.go` name, does
/// not count; nor does an include inside `#if` or a `#cgo darwin` flag. `vendor`, `testdata` and
/// the directories Go ignores are not read. `None` when the project builds anywhere (#248).
pub fn macos_only_cgo(root: &Path) -> Option<(String, String)> {
    let mut dirs = vec![root.to_path_buf()];
    let mut read = 0;
    let mut dirs_scanned = 0;
    while let Some(dir) = dirs.pop() {
        dirs_scanned += 1;
        if dirs_scanned > MACOS_CGO_DIR_SCAN_LIMIT {
            return None;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !is_ignored_dir(&name) {
                    dirs.push(entry.path());
                }
                continue;
            }
            // Test files cannot use cgo.
            if !kind.is_file()
                || !name.ends_with(".go")
                || name.ends_with("_test.go")
                || apple_only_file_name(&name)
            {
                continue;
            }
            read += 1;
            if read > MACOS_CGO_SCAN_LIMIT {
                return None;
            }
            let Ok(source) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Some(named) = apple_only_cgo(&source) {
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap_or(&path);
                return Some((relative.to_string_lossy().into_owned(), named));
            }
        }
    }
    None
}

/// Whether Go builds the file `name` only for Apple systems: `proc_darwin.go`,
/// `proc_darwin_arm64.go`.
fn apple_only_file_name(name: &str) -> bool {
    let stem = name.trim_end_matches(".go");
    let parts: Vec<&str> = stem.split('_').collect();
    let apple = |part: &&str| matches!(*part, "darwin" | "ios");
    match parts.as_slice() {
        [_, .., os] if apple(os) => true,
        [_, .., os, _arch] => apple(os),
        _ => false,
    }
}

/// The macOS-only header or framework the cgo preamble of a Go file names, when a Linux build
/// compiles the file.
fn apple_only_cgo(source: &str) -> Option<String> {
    let lines: Vec<&str> = source.lines().collect();
    for line in &lines {
        let line = line.trim();
        if line.starts_with("package ") {
            break;
        }
        if let Some(expr) = line.strip_prefix("//go:build")
            && !linux_satisfies(expr)
        {
            return None;
        }
    }
    let at = lines
        .iter()
        .position(|l| l.trim_start().starts_with("import \"C\""))?;
    // The preamble is the comment right above `import "C"`: a `/* */` block or `//` lines.
    let mut start = at;
    if lines[..at]
        .last()
        .is_some_and(|l| l.trim_end().ends_with("*/"))
    {
        while start > 0 {
            start -= 1;
            if lines[start].contains("/*") {
                break;
            }
        }
    } else {
        while start > 0 && lines[start - 1].trim_start().starts_with("//") {
            start -= 1;
        }
    }
    // An include inside `#if` is taken to be guarded for the platforms that have it.
    let mut guarded = 0usize;
    for line in &lines[start..at] {
        let line = line
            .trim()
            .trim_start_matches("//")
            .trim_start_matches("/*")
            .trim_end_matches("*/")
            .trim();
        if line.starts_with("#if") {
            guarded += 1;
        } else if line.starts_with("#endif") {
            guarded = guarded.saturating_sub(1);
        } else if guarded == 0
            && let Some(named) = apple_only_reference(line)
        {
            return Some(named);
        }
    }
    None
}

/// The macOS-only header a preamble line includes, or the framework it links on every system:
/// `#cgo LDFLAGS: -framework IOKit` does, `#cgo darwin LDFLAGS: -framework IOKit` only on macOS.
fn apple_only_reference(line: &str) -> Option<String> {
    if let Some(header) = line
        .strip_prefix("#include")
        .or_else(|| line.strip_prefix("#import"))
    {
        let header = header.trim().trim_start_matches(['<', '"']);
        let header = header.split(['>', '"']).next().unwrap_or(header);
        return APPLE_ONLY_HEADERS
            .iter()
            .any(|h| header.contains(h))
            .then(|| header.to_string());
    }
    let (name, flags) = line.strip_prefix("#cgo ")?.split_once(':')?;
    if name.trim().contains(' ') {
        return None;
    }
    let framework = flags
        .split_whitespace()
        .skip_while(|word| *word != "-framework")
        .nth(1)?;
    Some(format!("-framework {framework}"))
}

/// Whether a Linux build satisfies a `//go:build` expression such as `darwin && !ios`. The tags
/// a Linux cgo build sets are true, and so are Go release tags and the build nodes'
/// architectures; every other tag is false, as Go treats tags nobody set.
fn linux_satisfies(expr: &str) -> bool {
    fn any(tokens: &[&str], at: &mut usize) -> bool {
        let mut value = all(tokens, at);
        while tokens.get(*at) == Some(&"||") {
            *at += 1;
            value |= all(tokens, at);
        }
        value
    }
    fn all(tokens: &[&str], at: &mut usize) -> bool {
        let mut value = one(tokens, at);
        while tokens.get(*at) == Some(&"&&") {
            *at += 1;
            value &= one(tokens, at);
        }
        value
    }
    fn one(tokens: &[&str], at: &mut usize) -> bool {
        let Some(token) = tokens.get(*at).copied() else {
            return true;
        };
        *at += 1;
        match token {
            "!" => !one(tokens, at),
            "(" => {
                let value = any(tokens, at);
                if tokens.get(*at) == Some(&")") {
                    *at += 1;
                }
                value
            }
            tag => {
                matches!(tag, "linux" | "unix" | "cgo" | "gc" | "amd64" | "arm64")
                    || tag.starts_with("go1.")
            }
        }
    }
    let spaced = expr
        .replace('(', " ( ")
        .replace(')', " ) ")
        .replace('!', " ! ")
        .replace("&&", " && ")
        .replace("||", " || ");
    let tokens: Vec<&str> = spaced.split_whitespace().collect();
    any(&tokens, &mut 0)
}
