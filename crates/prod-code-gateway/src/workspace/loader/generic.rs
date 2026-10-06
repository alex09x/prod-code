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
use std::sync::Arc;

use prod_code_engine_generic::{GenericLspConfig, GenericLspEngine};

use crate::backend::BackendWorker;
use crate::workspace::probes::{wait_for_swift_build_settings, warm_cmake_compile_commands};

fn generic_lsp_config(engine: &str) -> Option<(GenericLspConfig, &'static str)> {
    match engine {
        "python" => Some((GenericLspConfig::for_python(), "Python")),
        "cpp" => Some((GenericLspConfig::for_cpp(), "C/C++ clangd")),
        "swift" => Some((GenericLspConfig::for_swift(), "Swift sourcekit-lsp")),
        "typescript" => Some((GenericLspConfig::for_typescript(), "TypeScript")),
        "java" => Some((GenericLspConfig::for_java(), "Java")),
        "kotlin" => Some((GenericLspConfig::for_kotlin(), "Kotlin")),
        "csharp" => Some((GenericLspConfig::for_csharp(), "C#")),
        "php" => Some((GenericLspConfig::for_php(), "PHP")),
        "ruby" => Some((GenericLspConfig::for_ruby(), "Ruby")),
        "dart" => Some((GenericLspConfig::for_dart(), "Dart")),
        "zig" => Some((GenericLspConfig::for_zig(), "Zig")),
        "elixir" => Some((GenericLspConfig::for_elixir(), "Elixir")),
        "scala" => Some((GenericLspConfig::for_scala(), "Scala")),
        "lua" => Some((GenericLspConfig::for_lua(), "Lua")),
        "haskell" => Some((GenericLspConfig::for_haskell(), "Haskell")),
        "ocaml" => Some((GenericLspConfig::for_ocaml(), "OCaml")),
        "clojure" => Some((GenericLspConfig::for_clojure(), "Clojure")),
        "julia" => Some((GenericLspConfig::for_julia(), "Julia")),
        "shell" => Some((GenericLspConfig::for_shell(), "Shell")),
        "r" => Some((GenericLspConfig::for_r(), "R")),
        "erlang" => Some((GenericLspConfig::for_erlang(), "Erlang")),
        "fsharp" => Some((GenericLspConfig::for_fsharp(), "F#")),
        "perl" => Some((GenericLspConfig::for_perl(), "Perl")),
        "solidity" => Some((GenericLspConfig::for_solidity(), "Solidity")),
        "nim" => Some((GenericLspConfig::for_nim(), "Nim")),
        "d" => Some((GenericLspConfig::for_d(), "D")),
        "fortran" => Some((GenericLspConfig::for_fortran(), "Fortran")),
        "sql" => Some((GenericLspConfig::for_sql(), "SQL")),
        "graphql" => Some((GenericLspConfig::for_graphql(), "GraphQL")),
        "protobuf" => Some((GenericLspConfig::for_protobuf(), "Protobuf")),
        "crystal" => Some((GenericLspConfig::for_crystal(), "Crystal")),
        "groovy" => Some((GenericLspConfig::for_groovy(), "Groovy")),
        "ada" => Some((GenericLspConfig::for_ada(), "Ada")),
        "v" => Some((GenericLspConfig::for_v(), "V")),
        "racket" => Some((GenericLspConfig::for_racket(), "Racket")),
        "terraform" => Some((GenericLspConfig::for_terraform(), "Terraform")),
        "nix" => Some((GenericLspConfig::for_nix(), "Nix")),
        "markdown" => Some((GenericLspConfig::for_markdown(), "Markdown")),
        "yaml" => Some((GenericLspConfig::for_yaml(), "YAML")),
        "toml" => Some((GenericLspConfig::for_toml(), "TOML")),
        "json" => Some((GenericLspConfig::for_json(), "JSON")),
        "html" => Some((GenericLspConfig::for_html(), "HTML")),
        "css" => Some((GenericLspConfig::for_css(), "CSS")),
        "dockerfile" => Some((GenericLspConfig::for_dockerfile(), "Dockerfile")),
        "svelte" => Some((GenericLspConfig::for_svelte(), "Svelte")),
        "vue" => Some((GenericLspConfig::for_vue(), "Vue")),
        "assembly" => Some((GenericLspConfig::for_assembly(), "Assembly")),
        _ => None,
    }
}

pub(crate) async fn load_generic(
    workspace_root: &Path,
    engine: &str,
) -> Option<(Option<Arc<GenericLspEngine>>, Option<Arc<BackendWorker>>)> {
    let (mut config, label) = generic_lsp_config(engine)?;

    if engine == "cpp" {
        warm_cmake_compile_commands(workspace_root).await;
    } else if engine == "swift" {
        for (k, v) in crate::swift_cache::swift_module_cache_env() {
            config.env.insert(k, v);
        }
    }

    match GenericLspEngine::spawn(workspace_root, config).await {
        Ok(generic_eng) => {
            tracing::info!(workspace = ?workspace_root, "Supervised GenericLspEngine ({label}) active");
            if engine == "swift" {
                wait_for_swift_build_settings(&generic_eng, workspace_root).await;
            }
            Some((Some(Arc::new(generic_eng)), None))
        }
        Err(err) => {
            tracing::warn!(error = %err, workspace = ?workspace_root, "Failed to spawn {label} LSP; falling back to subprocess");
            let backend = BackendWorker::spawn(workspace_root, engine)
                .await
                .ok()
                .map(Arc::new);
            Some((None, backend))
        }
    }
}
