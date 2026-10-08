/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

pub(crate) fn is_project_config_file(rel_path: &str) -> bool {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    matches!(
        name,
        "tsconfig.json"
            | "jsconfig.json"
            | "package.json"
            | "deno.json"
            | "pyproject.toml"
            | "setup.cfg"
            | "setup.py"
            | "pyrightconfig.json"
            | "uv.lock"
            | "CMakeLists.txt"
            | "compile_commands.json"
            | ".clangd"
            | "meson.build"
            | "Package.swift"
            | "Package.resolved"
            | "project.pbxproj"
            | "go.mod"
            | "go.work"
            | "Cargo.toml"
            | "Cargo.lock"
            | "rust-toolchain.toml"
            | "prod-code.toml"
    ) || name.starts_with("requirements")
}

/// LSP `languageId` for a server-side path, for the documents the gateway opens itself.
pub(crate) fn language_id_for_server_path(path: &std::path::Path) -> &'static str {
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
        "cpp" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "ixx" => "cpp",
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
        _ if name == "CMakeLists.txt"
            || path.extension().and_then(|e| e.to_str()) == Some("cmake") =>
        {
            "cmake"
        }
        _ if name == "Dockerfile" || name == "Containerfile" => "dockerfile",
        _ if name == "Makefile" || name == "makefile" || name == "GNUmakefile" => "makefile",
        _ => "plaintext",
    }
}

/// Runs `textDocument/rename` on a managed language server after opening every file that
/// references the symbol (pyright, for one, only rewrites open documents). Files the gateway
/// opened are closed again afterwards. Returns the server's response and how many files were
/// opened.
pub(crate) async fn rename_with_references_open(
    engine: &prod_code_engine_generic::GenericLspEngine,
    params: serde_json::Value,
) -> (anyhow::Result<serde_json::Value>, usize) {
    pub(crate) const MAX_OPENED: usize = 200;
    pub(crate) const MAX_FILE_BYTES: usize = 2 * 1024 * 1024;
    let own_uri = params
        .get("textDocument")
        .and_then(|t| t.get("uri"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let refs_params = serde_json::json!({
        "textDocument": params.get("textDocument").cloned().unwrap_or_default(),
        "position": params.get("position").cloned().unwrap_or_default(),
        "context": { "includeDeclaration": true },
    });
    let mut opened: Vec<String> = Vec::new();
    if let Ok(refs) = engine
        .send_request("textDocument/references", refs_params)
        .await
    {
        let uris: std::collections::BTreeSet<String> = refs
            .get("result")
            .and_then(|r| r.as_array())
            .map(|locations| {
                locations
                    .iter()
                    .filter_map(|l| l.get("uri").and_then(|u| u.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        for uri in uris.into_iter().filter(|u| *u != own_uri).take(MAX_OPENED) {
            let path = uri_or_path(&uri);
            let Ok(text) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            if text.len() > MAX_FILE_BYTES {
                continue;
            }
            let did_open = serde_json::json!({
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id_for_server_path(&path),
                    "version": 1,
                    "text": text,
                }
            });
            if engine
                .send_notification("textDocument/didOpen", did_open)
                .await
                .is_ok()
            {
                opened.push(uri);
            }
        }
    }
    let resp = engine.send_request("textDocument/rename", params).await;
    for uri in &opened {
        let _ = engine
            .send_notification(
                "textDocument/didClose",
                serde_json::json!({ "textDocument": { "uri": uri } }),
            )
            .await;
    }
    (resp, opened.len())
}
