/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::engine::detect::engine_at;
use crate::sync::engine::workspace::{
    excluded_from_root_workspace, is_in_dependency_dir, missing_path_is_confined_to,
};
use std::path::{Path, PathBuf};

/// Engine the gateway is expected to pick for `root` from its manifest, or `None` when the
/// checkout carries no manifest the gateway keys on. Mirrors the gateway's detection order.
/// The project a path belongs to inside a checkout: the nearest ancestor of `hint` (up to
/// `root`) carrying a manifest of a *different* language than the checkout root. Returns the
/// engine subpath (relative, `/`-separated) and that project's engine; `(None, root engine)`
/// when the path belongs to the root project (nested crates of one Cargo workspace stay
/// with the workspace).
pub fn engine_project(root: &Path, hint: &Path) -> (Option<String>, Option<&'static str>) {
    let root_engine = expected_engine(root);
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    // A caller may name a source relative to this checkout while its process runs elsewhere.
    // Resolve it against the supplied root before examining files or project manifests (#488).
    let hint = if let Ok(rel) = hint.strip_prefix(root) {
        canonical_root.join(rel)
    } else if hint.is_absolute() {
        hint.to_path_buf()
    } else {
        canonical_root.join(hint)
    };
    let mut dir = std::fs::canonicalize(&hint).unwrap_or(hint);
    let missing_path = matches!(
        std::fs::symlink_metadata(&dir),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound
    );
    if missing_path && !missing_path_is_confined_to(&canonical_root, &dir) {
        return (None, root_engine);
    }
    let existing_file = dir.is_file();
    let proposed_file = missing_path && engine_for_file(&dir).is_some();
    let file = (existing_file || proposed_file).then(|| dir.clone());
    if file.is_some() {
        dir = dir.parent().map(Path::to_path_buf).unwrap_or(dir);
    }
    let file_dir = dir.clone();
    let own_language = file.as_deref().and_then(engine_for_file);

    while dir.starts_with(&canonical_root) && dir != canonical_root {
        if !is_in_dependency_dir(&dir)
            && let Some(engine) = expected_engine(&dir)
        {
            // Same language is not the same project. A Cargo workspace answers for its
            // members; a crate it excludes belongs to no project the root analyzer loaded, so
            // it needs one of its own or every query in it comes back null.
            if Some(engine) == root_engine && !excluded_from_root_workspace(&canonical_root, &dir) {
                break;
            }
            // A file of another language inside the project (a Python script in a Swift
            // package) is none of its server's sources: it is a loose file, below (#362).
            // sourcekit-lsp does answer for the C family of a package's C targets.
            if let Some(own) = own_language
                && own != engine
                && !(engine == "swift" && own == "cpp")
            {
                break;
            }
            let rel = dir
                .strip_prefix(&canonical_root)
                .ok()
                .map(|r| {
                    r.components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .filter(|r| !r.is_empty());
            return (rel, Some(engine));
        }
        match dir.parent() {
            Some(parent) => dir = parent.to_path_buf(),
            None => break,
        }
    }
    // A loose file of another language, in a directory no project of its own claims (a
    // Python script in a Rust repository), is served by its own language's engine rooted at
    // its directory, not by the root's analyzer, which has no answer for it (#247). A file at
    // the root itself stays with the root: its engine cannot be keyed apart from the root's.
    if !is_in_dependency_dir(&file_dir)
        && let Some(own) = file.as_deref().and_then(engine_for_file)
        && Some(own) != root_engine
        && !matches!(own, "markdown" | "yaml" | "toml" | "json" | "html" | "css")
        && let Some(rel) = file_dir
            .strip_prefix(&canonical_root)
            .ok()
            .map(|r| {
                r.components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/")
            })
            .filter(|r| !r.is_empty())
    {
        return (Some(rel), Some(own));
    }
    (None, root_engine)
}

/// The checkout a local file outside `root` belongs to: the nearest ancestor that is a git
/// checkout, or else the nearest with a manifest the gateway keys on (as the command line picks
/// a file's checkout). `None` for a file under `root`, one that does not exist here (a path only
/// the node has), or one no checkout holds.
pub fn other_checkout(root: &Path, file: &Path) -> Option<PathBuf> {
    let canonical_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let file = std::fs::canonicalize(file).ok()?;
    if file.starts_with(&canonical_root) || is_in_dependency_dir(&file) {
        return None;
    }
    let mut manifest = None;
    for dir in file.ancestors().skip(1) {
        if is_in_dependency_dir(dir) {
            continue;
        }
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        if manifest.is_none() && expected_engine(dir).is_some() {
            manifest = Some(dir.to_path_buf());
        }
    }
    manifest
}

/// The engine a source file's extension names, when it names one.

pub fn engine_for_file(path: &Path) -> Option<&'static str> {
    let filename = path.file_name()?.to_str()?.to_ascii_lowercase();
    if filename.starts_with("dockerfile") || filename.starts_with("containerfile") {
        return Some("dockerfile");
    }
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if ext == "v"
        && path
            .ancestors()
            .any(|directory| directory.join("v.mod").is_file())
    {
        return Some("v");
    }
    Some(match ext.as_str() {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => "typescript",
        "astro" => "astro",
        "c" | "cc" | "cpp" | "cxx" | "h" | "hh" | "hpp" | "hxx" | "m" | "mm" => "cpp",
        "swift" => "swift",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "cs" => "csharp",
        "scala" | "sc" => "scala",
        "zig" => "zig",
        "php" => "php",
        "rb" => "ruby",
        "dart" => "dart",
        "ex" | "exs" => "elixir",
        "lua" => "lua",
        "hs" | "lhs" => "haskell",
        "ml" | "mli" => "ocaml",
        "clj" | "cljs" | "cljc" => "clojure",
        "jl" => "julia",
        "sh" | "bash" | "zsh" => "shell",
        "r" => "r",
        "erl" => "erlang",
        "fs" | "fsi" | "fsx" | "fsproj" => "fsharp",
        "pl" | "pm" => "perl",
        "sol" => "solidity",
        "nim" | "nimble" => "nim",
        "d" => "d",
        "f" | "for" | "f77" | "f90" | "f95" | "f03" | "f08" => "fortran",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" => "protobuf",
        "cr" => "crystal",
        "groovy" | "gvy" => "groovy",
        "gpr" | "adb" | "ads" => "ada",
        "vsh" => "v",
        "rkt" => "racket",
        "tf" | "tofu" => "terraform",
        "nix" => "nix",
        "md" | "markdown" => "markdown",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "json" | "jsonc" => "json",
        "html" | "htm" => "html",
        "css" | "scss" | "less" => "css",
        "svelte" => "svelte",
        "vue" => "vue",
        "asm" | "nasm" | "s" => "assembly",
        "ps1" | "psm1" | "psd1" => "powershell",
        "bzl" | "star" => "starlark",
        "hcl" => "hcl",
        "typ" => "typst",
        "wat" | "wast" => "wat",
        "sv" | "svh" => "systemverilog",
        "vhd" | "vhdl" => "vhdl",
        "bal" => "ballerina",
        "jsonnet" | "libsonnet" => "jsonnet",
        "cue" => "cue",
        _ => return None,
    })
}

pub fn expected_engine(root: &Path) -> Option<&'static str> {
    if let Some(engine) = engine_at(root) {
        return Some(engine);
    }
    // A repository often keeps its project one directory down (`project/go.mod`,
    // `server/Cargo.toml`). Look one level deep and accept the answer only when every
    // child that has a manifest agrees, so a polyglot monorepo stays "any engine".
    let mut found: Option<&'static str> = None;
    let Ok(entries) = std::fs::read_dir(root) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !path.is_dir()
            || name.starts_with('.')
            || matches!(
                name.as_ref(),
                "target" | "node_modules" | "vendor" | "build" | "dist"
            )
        {
            continue;
        }
        match (engine_at(&path), found) {
            (Some(engine), None) => found = Some(engine),
            (Some(engine), Some(seen)) if engine != seen => return None,
            _ => {}
        }
    }
    found
}
