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

use super::local_go::{LocalDependencies, read_source};

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
/// the directories Go ignores are not read. Workspaces retain their recursive scan; wrappers
/// discover child modules, then exclude independent descendants of each selected module.
/// Imported, required local replacements contained in the scan root remain dependencies.
/// `None` when the project builds anywhere (#248).
pub fn macos_only_cgo(root: &Path) -> Option<(String, String)> {
    let root = root.canonicalize().ok()?;
    // A workspace or wrapper is not a selected module. Discover its child projects before
    // enforcing module boundaries; a go.work may also sit beside a root go.mod.
    let workspace = root.join("go.work").is_file();
    let in_module = root.join("go.mod").is_file() && !workspace;
    let mut owners = Vec::new();
    if in_module {
        owners.push(LocalDependencies::new(&root, &root));
    }
    let mut dirs = vec![(root.clone(), in_module.then_some(0), true)];
    let mut visited = std::collections::HashSet::new();
    let mut read = 0;
    let mut dirs_scanned = 0;
    while let Some((dir, mut owner, selected)) = dirs.pop() {
        // A shared dependency's imports must expand under each selected owner's replace table.
        // The shared directory/file budgets still bound these owner-specific visits.
        if !visited.insert((dir.clone(), owner, selected)) {
            continue;
        }
        dirs_scanned += 1;
        if dirs_scanned > MACOS_CGO_DIR_SCAN_LIMIT {
            return None;
        }
        if !workspace && owner.is_none() && dir.join("go.mod").is_file() {
            owner = Some(owners.len());
            owners.push(LocalDependencies::new(&dir, &root));
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
                let child_module = entry.path().join("go.mod").is_file();
                if !is_ignored_dir(&name) && !(owner.is_some() && child_module) {
                    dirs.push((entry.path(), owner, selected));
                }
                continue;
            }
            if !kind.is_file() || !linux_file_name(&name) {
                continue;
            }
            read += 1;
            if read > MACOS_CGO_SCAN_LIMIT {
                return None;
            }
            let Some(source) = read_source(&entry.path()) else {
                continue;
            };
            if !linux_source(&source) {
                continue;
            }
            // Testing selected packages does not execute their dependencies' tests.
            if name.ends_with("_test.go") && !selected {
                continue;
            }
            if let Some(owner) = owner {
                for dependency in owners[owner].imported_roots(&source) {
                    dirs.push((dependency, Some(owner), false));
                }
            }
            // Tests cannot use cgo directly, but may import packages that do.
            if name.ends_with("_test.go") {
                continue;
            }
            if let Some(named) = apple_only_cgo(&source) {
                let path = entry.path();
                let relative = path.strip_prefix(&root).unwrap_or(&path);
                return Some((relative.to_string_lossy().into_owned(), named));
            }
        }
    }
    None
}

/// Go ignores dot/underscore files and recognizes only known OS/architecture suffixes.
/// Match the Linux architectures used by `linux_satisfies`, without probing a toolchain.
fn linux_file_name(name: &str) -> bool {
    if name.starts_with(['.', '_']) {
        return false;
    }
    let Some(stem) = name.strip_suffix(".go") else {
        return false;
    };
    let stem = stem.strip_suffix("_test").unwrap_or(stem);
    let Some((prefix, suffix)) = stem.rsplit_once('_') else {
        return true;
    };
    let known_arch = matches!(
        suffix,
        "386"
            | "amd64"
            | "amd64p32"
            | "arm"
            | "armbe"
            | "arm64"
            | "arm64be"
            | "loong64"
            | "mips"
            | "mipsle"
            | "mips64"
            | "mips64le"
            | "mips64p32"
            | "mips64p32le"
            | "ppc"
            | "ppc64"
            | "ppc64le"
            | "riscv"
            | "riscv64"
            | "s390"
            | "s390x"
            | "sparc"
            | "sparc64"
            | "wasm"
    );
    let os = if known_arch {
        if !matches!(suffix, "amd64" | "arm64") {
            return false;
        }
        prefix.rsplit_once('_').map(|(_, os)| os).unwrap_or("")
    } else {
        suffix
    };
    !matches!(
        os,
        "aix"
            | "android"
            | "darwin"
            | "dragonfly"
            | "freebsd"
            | "hurd"
            | "illumos"
            | "ios"
            | "js"
            | "nacl"
            | "netbsd"
            | "openbsd"
            | "plan9"
            | "solaris"
            | "wasip1"
            | "windows"
            | "zos"
    )
}

/// The macOS-only header or framework the cgo preamble of a Go file names, when a Linux build
/// compiles the file.
fn apple_only_cgo(source: &str) -> Option<String> {
    if !linux_source(source) {
        return None;
    }
    let lines: Vec<&str> = source.lines().collect();
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

fn linux_source(source: &str) -> bool {
    source
        .lines()
        .map(str::trim)
        .take_while(|line| !line.starts_with("package "))
        .all(|line| line.strip_prefix("//go:build").is_none_or(linux_satisfies))
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
