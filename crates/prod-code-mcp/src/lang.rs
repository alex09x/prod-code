/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! LSP `languageId` for a file path, shared by the CLI and the MCP tools.

use std::path::Path;

/// The LSP `languageId` a file is opened with, from its extension.
pub fn language_id_for_path(path: &Path) -> &'static str {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "rust",
        "go" => "go",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "astro" => "astro",
        "svelte" => "svelte",
        "vue" => "vue",
        "html" | "htm" => "html",
        "css" | "scss" | "sass" | "less" => "css",
        "xml" | "svg" => "xml",
        "c" | "h" => "c",
        "cpp" | "hpp" | "cc" | "cxx" | "hh" | "hxx" | "ixx" => "cpp",
        "m" => "objective-c",
        "mm" => "objective-cpp",
        "swift" => "swift",
        "cs" => "csharp",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "scala" | "sc" => "scala",
        "php" | "phtml" => "php",
        "rb" | "erb" | "rake" | "gemspec" => "ruby",
        "zig" | "zon" => "zig",
        "dart" => "dart",
        "lua" => "lua",
        "hs" | "lhs" => "haskell",
        "ml" | "mli" => "ocaml",
        "ex" | "exs" => "elixir",
        "clj" | "cljs" | "cljc" | "edn" => "clojure",
        "jl" => "julia",
        "r" | "R" | "Rmd" => "r",
        "erl" | "hrl" => "erlang",
        "fs" | "fsi" | "fsx" => "fsharp",
        "pl" | "pm" => "perl",
        "sol" => "solidity",
        "nim" | "nims" | "nimble" => "nim",
        "d" | "di" => "d",
        "f" | "for" | "f90" | "f95" | "f03" | "f08" => "fortran",
        "cr" => "crystal",
        "groovy" | "gvy" | "gy" | "gsh" | "gradle" => "groovy",
        "adb" | "ads" => "ada",
        "v" | "vh" => "v",
        "rkt" => "racket",
        "tf" | "tfvars" => "terraform",
        "hcl" => "hcl",
        "nix" => "nix",
        "s" | "S" | "asm" => "assembly",
        "sql" => "sql",
        "graphql" | "gql" => "graphql",
        "proto" => "proto",
        "thrift" => "thrift",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "sh" | "bash" | "zsh" => "shellscript",
        "ps1" | "psm1" | "psd1" => "powershell",
        "bzl" | "star" => "starlark",
        "typ" => "typst",
        "wat" | "wast" => "wat",
        "wasm" => "wasm",
        "sv" | "svh" => "systemverilog",
        "vhd" | "vhdl" => "vhdl",
        "bal" => "ballerina",
        "jsonnet" | "libsonnet" => "jsonnet",
        "cue" => "cue",
        _ if matches!(name, "BUILD" | "BUILD.bazel" | "WORKSPACE" | "WORKSPACE.bazel" | "MODULE.bazel" | "Tiltfile") => "starlark",
        _ if name == "CMakeLists.txt" || path.extension().and_then(|e| e.to_str()) == Some("cmake") => "cmake",
        _ if name == "Dockerfile" || name == "Containerfile" => "dockerfile",
        _ if name == "Makefile" || name == "makefile" || name == "GNUmakefile" => "makefile",
        _ => "plaintext",
    }
}

/// Whether a file is a C or C++ header: what callers include to learn a declaration, and so
/// where a type they need has to be declared.
pub fn is_header(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        "h" | "hh" | "hpp" | "hxx" | "inl"
    )
}

/// Groups file languages by LSP engine compatibility so that mixed-language multi-file batches
/// can be routed to the appropriate engine rather than failing against an incompatible one (#751, #761).
pub fn engine_group_for_path(path: &Path) -> &'static str {
    match language_id_for_path(path) {
        "c" | "cpp" | "objective-c" | "objective-cpp" => "cpp",
        "typescript" | "typescriptreact" | "javascript" | "javascriptreact" => "typescript",
        "rust" => "rust",
        "go" => "go",
        "python" => "python",
        "swift" => "swift",
        "csharp" => "csharp",
        "java" => "java",
        "kotlin" => "kotlin",
        "scala" => "scala",
        "php" => "php",
        "ruby" => "ruby",
        "zig" => "zig",
        "dart" => "dart",
        "lua" => "lua",
        "elixir" => "elixir",
        "json" => "json",
        "markdown" => "markdown",
        "xml" => "xml",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_are_told_from_sources() {
        assert!(is_header(Path::new("src/home.h")));
        assert!(is_header(Path::new("include/shapes/home.hpp")));
        assert!(!is_header(Path::new("src/home.c")));
        assert!(!is_header(Path::new("src/home.cpp")));
    }

    #[test]
    fn maps_common_extensions() {
        assert_eq!(language_id_for_path(Path::new("src/lib.rs")), "rust");
        assert_eq!(
            language_id_for_path(Path::new("src/index.ts")),
            "typescript"
        );
        assert_eq!(language_id_for_path(Path::new("src/util.h")), "c");
        assert_eq!(
            language_id_for_path(Path::new("Sources/App/main.swift")),
            "swift"
        );
        assert_eq!(language_id_for_path(Path::new("src/App.astro")), "astro");
        assert_eq!(language_id_for_path(Path::new("src/App.svelte")), "svelte");
        assert_eq!(language_id_for_path(Path::new("src/App.vue")), "vue");
        assert_eq!(language_id_for_path(Path::new("src/main.zig")), "zig");
        assert_eq!(language_id_for_path(Path::new("src/app.dart")), "dart");
        assert_eq!(language_id_for_path(Path::new("src/script.lua")), "lua");
        assert_eq!(language_id_for_path(Path::new("src/Main.hs")), "haskell");
        assert_eq!(language_id_for_path(Path::new("src/Program.cs")), "csharp");
        assert_eq!(language_id_for_path(Path::new("src/App.java")), "java");
        assert_eq!(language_id_for_path(Path::new("src/App.kt")), "kotlin");
        assert_eq!(language_id_for_path(Path::new("src/index.php")), "php");
        assert_eq!(language_id_for_path(Path::new("src/app.rb")), "ruby");
        assert_eq!(language_id_for_path(Path::new("src/lib.ex")), "elixir");
        assert_eq!(language_id_for_path(Path::new("src/core.clj")), "clojure");
        assert_eq!(language_id_for_path(Path::new("src/math.jl")), "julia");
        assert_eq!(language_id_for_path(Path::new("src/analysis.r")), "r");
        assert_eq!(language_id_for_path(Path::new("src/contract.sol")), "solidity");
        assert_eq!(language_id_for_path(Path::new("src/query.sql")), "sql");
        assert_eq!(language_id_for_path(Path::new("CMakeLists.txt")), "cmake");
        assert_eq!(language_id_for_path(Path::new("Dockerfile")), "dockerfile");
        assert_eq!(language_id_for_path(Path::new("Makefile")), "makefile");
        assert_eq!(language_id_for_path(Path::new("src/main.fs")), "fsharp");
        assert_eq!(language_id_for_path(Path::new("scripts/setup.ps1")), "powershell");
        assert_eq!(language_id_for_path(Path::new("BUILD.bazel")), "starlark");
        assert_eq!(language_id_for_path(Path::new("rules/def.bzl")), "starlark");
        assert_eq!(language_id_for_path(Path::new("terragrunt.hcl")), "hcl");
        assert_eq!(language_id_for_path(Path::new("paper.typ")), "typst");
        assert_eq!(language_id_for_path(Path::new("module.wat")), "wat");
        assert_eq!(language_id_for_path(Path::new("module.wasm")), "wasm");
        assert_eq!(language_id_for_path(Path::new("core.sv")), "systemverilog");
        assert_eq!(language_id_for_path(Path::new("alu.vhd")), "vhdl");
        assert_eq!(language_id_for_path(Path::new("main.bal")), "ballerina");
        assert_eq!(language_id_for_path(Path::new("service.jsonnet")), "jsonnet");
        assert_eq!(language_id_for_path(Path::new("config.cue")), "cue");
        assert_eq!(language_id_for_path(Path::new("README")), "plaintext");
    }

    #[test]
    fn engine_groups_partition_mixed_languages() {
        assert_eq!(engine_group_for_path(Path::new("cmd/check/main.go")), "go");
        assert_eq!(engine_group_for_path(Path::new("scripts/check.py")), "python");
        assert_eq!(engine_group_for_path(Path::new("Tests/Test.swift")), "swift");
        assert_eq!(engine_group_for_path(Path::new("src/main.rs")), "rust");
        assert_eq!(engine_group_for_path(Path::new("include/util.h")), "cpp");
        assert_eq!(engine_group_for_path(Path::new("src/util.cpp")), "cpp");
        assert_eq!(engine_group_for_path(Path::new("src/app.ts")), "typescript");
        assert_eq!(engine_group_for_path(Path::new("src/app.jsx")), "typescript");
        assert_eq!(engine_group_for_path(Path::new("manifest.json")), "json");
    }
}
