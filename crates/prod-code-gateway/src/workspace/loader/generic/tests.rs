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

#[test]
fn every_managed_language_has_its_label_and_nothing_else_does() {
    let labels = [
        ("python", "Python"),
        ("cpp", "C/C++ clangd"),
        ("swift", "Swift sourcekit-lsp"),
        ("typescript", "TypeScript"),
        ("java", "Java"),
        ("kotlin", "Kotlin"),
        ("csharp", "C#"),
        ("php", "PHP"),
        ("ruby", "Ruby"),
        ("dart", "Dart"),
        ("zig", "Zig"),
        ("elixir", "Elixir"),
        ("scala", "Scala"),
        ("lua", "Lua"),
        ("haskell", "Haskell"),
        ("ocaml", "OCaml"),
        ("clojure", "Clojure"),
        ("julia", "Julia"),
        ("shell", "Shell"),
        ("r", "R"),
        ("erlang", "Erlang"),
        ("fsharp", "F#"),
        ("perl", "Perl"),
        ("solidity", "Solidity"),
        ("nim", "Nim"),
        ("d", "D"),
        ("fortran", "Fortran"),
        ("sql", "SQL"),
        ("graphql", "GraphQL"),
        ("protobuf", "Protobuf"),
        ("crystal", "Crystal"),
        ("groovy", "Groovy"),
        ("ada", "Ada"),
        ("v", "V"),
        ("racket", "Racket"),
        ("terraform", "Terraform"),
        ("nix", "Nix"),
        ("markdown", "Markdown"),
        ("yaml", "YAML"),
        ("toml", "TOML"),
        ("json", "JSON"),
        ("html", "HTML"),
        ("css", "CSS"),
        ("dockerfile", "Dockerfile"),
        ("svelte", "Svelte"),
        ("vue", "Vue"),
        ("assembly", "Assembly"),
    ];
    for (engine, label) in labels {
        let (_, got) = generic_lsp_config(engine).unwrap_or_else(|| panic!("{engine} unmanaged"));
        assert_eq!(got, label, "{engine}");
    }
    // The Rust and Go engines have their own loaders, and names are matched exactly.
    for other in ["rust", "go", "", "Python", "python ", "unknown", "c++"] {
        assert!(generic_lsp_config(other).is_none(), "{other:?}");
    }
}

#[tokio::test]
async fn an_unmanaged_engine_loads_nothing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_generic(dir.path(), "rust").await.is_none());
    assert!(load_generic(dir.path(), "").await.is_none());
}

/// A language server that is not installed leaves neither an engine nor a worker: the
/// subprocess fallback only exists for the Rust and Go servers.
#[tokio::test]
async fn a_server_that_cannot_start_leaves_no_engine_and_no_worker() {
    let dir = tempfile::tempdir().unwrap();
    match load_generic(dir.path(), "ada").await {
        Some((engine, worker)) => {
            assert!(engine.is_none(), "no ada language server on a build node");
            assert!(worker.is_none());
        }
        None => panic!("ada is a managed language"),
    }
}
