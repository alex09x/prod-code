/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use serde_json::json;
use std::fs;

#[test]
fn language_ids_and_project_configs_are_exact_and_case_sensitive() {
    for (extensions, expected) in [
        ("rs", "rust"),
        ("go", "go"),
        ("py pyi", "python"),
        ("ts mts cts", "typescript"),
        ("tsx", "typescriptreact"),
        ("js mjs cjs", "javascript"),
        ("jsx", "javascriptreact"),
        ("astro", "astro"),
        ("svelte", "svelte"),
        ("vue", "vue"),
        ("html htm", "html"),
        ("css scss sass less", "css"),
        ("xml svg", "xml"),
        ("c h", "c"),
        ("cpp cc cxx hpp hh hxx ixx", "cpp"),
        ("m", "objective-c"),
        ("mm", "objective-cpp"),
        ("swift", "swift"),
        ("cs", "csharp"),
        ("java", "java"),
        ("kt kts", "kotlin"),
        ("scala sc", "scala"),
        ("php phtml", "php"),
        ("rb erb rake gemspec", "ruby"),
        ("zig zon", "zig"),
        ("dart", "dart"),
        ("lua", "lua"),
        ("hs lhs", "haskell"),
        ("ml mli", "ocaml"),
        ("ex exs", "elixir"),
        ("clj cljs cljc edn", "clojure"),
        ("jl", "julia"),
        ("r R Rmd", "r"),
        ("erl hrl", "erlang"),
        ("pl pm", "perl"),
        ("sol", "solidity"),
        ("nim nims nimble", "nim"),
        ("d di", "d"),
        ("f for f90 f95 f03 f08", "fortran"),
        ("cr", "crystal"),
        ("groovy gvy gy gsh gradle", "groovy"),
        ("adb ads", "ada"),
        ("v vh", "v"),
        ("rkt", "racket"),
        ("tf tfvars", "terraform"),
        ("nix", "nix"),
        ("s S asm", "assembly"),
        ("sql", "sql"),
        ("graphql gql", "graphql"),
        ("proto", "proto"),
        ("thrift", "thrift"),
        ("toml", "toml"),
        ("json", "json"),
        ("yaml yml", "yaml"),
        ("md markdown", "markdown"),
        ("sh bash zsh", "shellscript"),
        ("cmake", "cmake"),
        ("unknown RS", "plaintext"),
    ] {
        for ext in extensions.split_whitespace() {
            assert_eq!(
                language_id_for_server_path(Path::new(&format!("nested/file.{ext}"))),
                expected,
                "{ext}"
            );
        }
    }
    for (name, expected) in [
        ("CMakeLists.txt", "cmake"),
        ("Dockerfile", "dockerfile"),
        ("Containerfile", "dockerfile"),
        ("Makefile", "makefile"),
        ("makefile", "makefile"),
        ("GNUmakefile", "makefile"),
        ("", "plaintext"),
    ] {
        assert_eq!(language_id_for_server_path(Path::new(name)), expected);
    }
    for name in [
        "tsconfig.json",
        "jsconfig.json",
        "package.json",
        "deno.json",
        "pyproject.toml",
        "setup.cfg",
        "setup.py",
        "pyrightconfig.json",
        "uv.lock",
        "CMakeLists.txt",
        "compile_commands.json",
        ".clangd",
        "meson.build",
        "Package.swift",
        "Package.resolved",
        "project.pbxproj",
        "go.mod",
        "go.work",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "prod-code.toml",
        "requirements-dev.txt",
    ] {
        assert!(is_project_config_file(&format!("nested/{name}")), "{name}");
    }
    for name in [
        "",
        "file.rs",
        "Cargo.toml.bak",
        "PACKAGE.JSON",
        "requirements/subdir",
    ] {
        assert!(!is_project_config_file(name), "{name}");
    }
}

#[tokio::test]
async fn rename_opens_unique_readable_references_and_closes_them_even_on_refusal() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("reference.py");
    fs::write(&path, "name = 'é😀'\r\n").unwrap();
    let own = file_uri(&temp.path().join("own.py"));
    let reference = file_uri(&path);
    let oversized = temp.path().join("oversized.py");
    fs::write(&oversized, vec![b' '; 2 * 1024 * 1024 + 1]).unwrap();
    let invalid = temp.path().join("invalid.py");
    fs::write(&invalid, [0xff]).unwrap();
    let engine = crate::lsp::test_support::server(temp.path(), json!({
        "textDocument/references":{"result":[{"uri":own},{"uri":reference},{"uri":reference},{"uri":file_uri(&oversized)},{"uri":file_uri(&invalid)},{"uri":file_uri(&temp.path().join("missing.py"))},{"uri":42},{}]},
        "textDocument/rename":{"error":{"code":-32602,"message":"rename refused"}}
    })).await;
    let (response, count) = rename_with_references_open(
        &engine,
        json!({"textDocument":{"uri":own},"position":{"line":0,"character":0},"newName":"renamed"}),
    )
    .await;
    assert_eq!(count, 1);
    assert_eq!(response.unwrap()["error"]["message"], "rename refused");
    let events = engine.send_request("test/events", json!({})).await.unwrap();
    let relevant: Vec<_> = events["result"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| {
            matches!(
                v["method"].as_str(),
                Some("textDocument/didOpen" | "textDocument/didClose")
            )
        })
        .cloned()
        .collect();
    assert_eq!(
        relevant,
        vec![
            json!({"method":"textDocument/didOpen","params":{"textDocument":{"uri":reference,"languageId":"python","version":1,"text":"name = 'é😀'\r\n"}}}),
            json!({"method":"textDocument/didClose","params":{"textDocument":{"uri":reference}}})
        ]
    );
}

#[tokio::test]
async fn malformed_reference_results_still_forward_rename() {
    for refs in [
        json!({"result":null}),
        json!({"error":{"code":-1,"message":"no references"}}),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let edit = json!({"changes":{}});
        let engine = crate::lsp::test_support::server(
            temp.path(),
            json!({"textDocument/references":refs,"textDocument/rename":{"result":edit}}),
        )
        .await;
        let (response, count) = rename_with_references_open(&engine, json!({})).await;
        assert_eq!(count, 0);
        assert_eq!(response.unwrap()["result"], edit);
    }
}
