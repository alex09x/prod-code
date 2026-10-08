/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::filter_path::is_fixture_path;
use crate::sync::scan::is_binary_or_media_file;
use std::path::Path;

/// Returns true if the relative path represents a code or configuration file relevant to language servers.
pub fn is_relevant_code_or_manifest_file(rel_path: &str) -> bool {
    let path = Path::new(rel_path);
    // Build and tool manifests whose extension alone would not qualify them.
    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if matches!(
        file_name,
        "CMakeLists.txt"
            | "CMakePresets.json"
            | "compile_commands.json"
            | "meson.build"
            | "meson_options.txt"
            | "requirements.txt"
            | "requirements-dev.txt"
            | "constraints.txt"
            | "pytest.ini"
            | "tox.ini"
            | "setup.cfg"
            | "mypy.ini"
            | "project.pbxproj"
            | "Podfile"
            | "Package.resolved"
            | ".clangd"
            | ".clang-format"
            | ".clang-tidy"
            | "Pipfile"
            | "BUILD"
            | "WORKSPACE"
            | "cabal.project"
            | "stack.yaml"
            | "package.yaml"
            | "dune-project"
            | "dune"
            | "deps.edn"
            | "DESCRIPTION"
            | "NAMESPACE"
            | "rebar.config"
            | "rebar.lock"
            | "erlang.mk"
            | "cpanfile"
            | "Makefile.PL"
            | "Build.PL"
            | "dist.ini"
            | "nim.cfg"
            | "dub.json"
            | "dub.sdl"
            | "rustc-wrapper"
            | "rustc_wrapper"
            | "cargo-wrapper"
    ) || (file_name.starts_with("requirements") && file_name.ends_with(".txt"))
        || file_name.ends_with(".sh")
    {
        return true;
    }

    // 1. Check directory components for non-code / build / data trees
    let mut under_code_dir = false;
    for component in path.components() {
        if let std::path::Component::Normal(comp) = component {
            let s = comp.to_string_lossy();
            if s.starts_with('.') && s != ".cargo" {
                return false;
            }
            if matches!(
                s.as_ref(),
                "src"
                    | "server"
                    | "client"
                    | "internal"
                    | "pkg"
                    | "cmd"
                    | "api"
                    | "Sources"
                    | "Tests"
                    | "include"
                    | "lib"
                    | "examples"
                    | "tests"
                    | "test"
                    | "fixtures"
                    | "testdata"
                    | "benches"
            ) {
                under_code_dir = true;
            }
            if !under_code_dir
                && matches!(
                    s.as_ref(),
                    "target"
                        | "node_modules"
                        | "vendor"
                        | "dist"
                        | "build"
                        | "results"
                        | "samples"
                        | "__pycache__"
                        | "artifacts"
                        | "dogfood-output"
                        | "data"
                        | "dataset"
                        | "datasets"
                        | "corpus"
                        | "traces"
                        | "state"
                        | "research"
                        | "benchmarks"
                        | "benchmark"
                        | ".idea"
                        | ".vscode"
                )
            {
                return false;
            }
            if under_code_dir && matches!(s.as_ref(), "node_modules" | "__pycache__") {
                return false;
            }
        }
    }

    // 2. Binary / media extensions (fixtures exempt from binary exclusion)
    if is_binary_or_media_file(rel_path) && !is_fixture_path(path) {
        return false;
    }

    // 3. Known non-code data, dumps, documentation, and log formats
    let lower = rel_path.to_lowercase();
    if lower.ends_with(".jsonl")
        || lower.ends_with(".csv")
        || lower.ends_with(".tsv")
        || lower.ends_with(".parquet")
        || lower.ends_with(".arrow")
        || lower.ends_with(".feather")
        || lower.ends_with(".log")
        || lower.ends_with(".md")
        || lower.ends_with(".txt")
        || lower.ends_with(".rst")
        || lower.ends_with(".pdf")
        || lower.ends_with(".doc")
        || lower.ends_with(".docx")
    {
        return false;
    }

    // 4. Code & manifest extensions
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();
        matches!(
            ext_lower.as_str(),
            "rs" | "go"
                | "mod"
                | "sum"
                | "work"
                | "py"
                | "pyi"
                | "js"
                | "mjs"
                | "cjs"
                | "jsx"
                | "ts"
                | "mts"
                | "cts"
                | "tsx"
                | "astro"
                | "vue"
                | "svelte"
                | "c"
                | "h"
                | "cc"
                | "cpp"
                | "cxx"
                | "hh"
                | "hpp"
                | "hxx"
                | "inl"
                | "java"
                | "kt"
                | "kts"
                | "fsproj"
                | "cabal"
                | "nimble"
                | "scala"
                | "sc"
                | "cs"
                | "swift"
                | "php"
                | "phtml"
                | "rb"
                | "erb"
                | "rake"
                | "gemspec"
                | "zig"
                | "zon"
                | "dart"
                | "lua"
                | "hs"
                | "lhs"
                | "ml"
                | "mli"
                | "ex"
                | "exs"
                | "clj"
                | "cljs"
                | "cljc"
                | "edn"
                | "jl"
                | "r"
                | "erl"
                | "hrl"
                | "pl"
                | "pm"
                | "sol"
                | "nim"
                | "nims"
                | "d"
                | "di"
                | "f"
                | "for"
                | "f90"
                | "f95"
                | "f03"
                | "f08"
                | "cr"
                | "groovy"
                | "gvy"
                | "gy"
                | "gsh"
                | "gradle"
                | "adb"
                | "ads"
                | "v"
                | "vh"
                | "rkt"
                | "tf"
                | "tfvars"
                | "nix"
                | "s"
                | "asm"
                | "proto"
                | "thrift"
                | "graphql"
                | "gql"
                | "sql"
                | "sh"
                | "bash"
                | "zsh"
                | "html"
                | "htm"
                | "css"
                | "scss"
                | "sass"
                | "less"
                | "xml"
                | "svg"
                | "cmake"
                | "toml"
                | "lock" // Cargo.lock, yarn.lock, poetry.lock: pin what the server builds
                | "yaml"
                | "yml"
                | "json"
        )
    } else {
        // Files without extension: manifests and scripts
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        matches!(
            file_name,
            "Makefile"
                | "Dockerfile"
                | "Containerfile"
                | "Procfile"
                | "Gemfile"
                | "Rakefile"
                | "Cargo.lock"
                | ".clangd"
                | ".clang-format"
                | ".clang-tidy"
                | "Pipfile"
                | "BUILD"
                | "WORKSPACE"
        )
    }
}
