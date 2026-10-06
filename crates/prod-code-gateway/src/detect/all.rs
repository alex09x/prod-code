/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::messages::EngineKind;
use std::path::Path;

use super::heuristics::*;
use super::markers::*;

/// Detect all applicable engine kinds for a workspace (e.g. polyglot monorepos).
pub fn detect_all_engines(root: &Path) -> Vec<EngineKind> {
    let mut engines = Vec::new();

    if RUST_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Rust);
    }
    if GO_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Go);
    }
    if PYTHON_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Python);
    }
    if TYPESCRIPT_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::TypeScript);
    }
    if CPP_MARKERS.iter().any(|m| root.join(m).exists()) || is_make_cpp_project(root) {
        engines.push(EngineKind::Cpp);
    }
    if has_swift_project(root) {
        engines.push(EngineKind::Swift);
    }
    if has_kotlin_project(root) {
        engines.push(EngineKind::Kotlin);
    }
    if has_java_project(root) {
        engines.push(EngineKind::Java);
    }
    if has_csharp_project(root) {
        engines.push(EngineKind::Csharp);
    }
    if PHP_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Php);
    }
    if RUBY_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Ruby);
    }
    if DART_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Dart);
    }
    if ZIG_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Zig);
    }
    if ELIXIR_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Elixir);
    }
    if SCALA_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Scala);
    }
    if has_lua_project(root) {
        engines.push(EngineKind::Lua);
    }
    if has_haskell_project(root) {
        engines.push(EngineKind::Haskell);
    }
    if has_ocaml_project(root) {
        engines.push(EngineKind::Ocaml);
    }
    if has_clojure_project(root) {
        engines.push(EngineKind::Clojure);
    }
    if has_julia_project(root) {
        engines.push(EngineKind::Julia);
    }
    if has_shell_project(root) {
        engines.push(EngineKind::Shell);
    }
    if has_r_project(root) {
        engines.push(EngineKind::R);
    }
    if has_erlang_project(root) {
        engines.push(EngineKind::Erlang);
    }
    if has_fsharp_project(root) {
        engines.push(EngineKind::Fsharp);
    }
    if has_perl_project(root) {
        engines.push(EngineKind::Perl);
    }
    if has_solidity_project(root) {
        engines.push(EngineKind::Solidity);
    }
    if has_nim_project(root) {
        engines.push(EngineKind::Nim);
    }
    if has_d_project(root) {
        engines.push(EngineKind::D);
    }
    if has_fortran_project(root) {
        engines.push(EngineKind::Fortran);
    }
    if has_sql_project(root) {
        engines.push(EngineKind::Sql);
    }
    if has_graphql_project(root) {
        engines.push(EngineKind::Graphql);
    }
    if has_protobuf_project(root) {
        engines.push(EngineKind::Protobuf);
    }
    if has_crystal_project(root) {
        engines.push(EngineKind::Crystal);
    }
    if has_groovy_project(root) || has_groovy_gradle_sources(root) {
        engines.push(EngineKind::Groovy);
    }
    if has_ada_project(root) {
        engines.push(EngineKind::Ada);
    }
    if has_v_project(root) {
        engines.push(EngineKind::V);
    }
    if has_racket_project(root) {
        engines.push(EngineKind::Racket);
    }
    if has_terraform_project(root) {
        engines.push(EngineKind::Terraform);
    }
    if has_nix_project(root) {
        engines.push(EngineKind::Nix);
    }
    if has_markdown_project(root) {
        engines.push(EngineKind::Markdown);
    }
    if has_yaml_project(root) {
        engines.push(EngineKind::Yaml);
    }
    if has_toml_project(root) {
        engines.push(EngineKind::Toml);
    }
    if has_json_project(root) {
        engines.push(EngineKind::Json);
    }
    if has_html_project(root) {
        engines.push(EngineKind::Html);
    }
    if has_css_project(root) {
        engines.push(EngineKind::Css);
    }
    if has_dockerfile_project(root) {
        engines.push(EngineKind::Dockerfile);
    }
    if has_svelte_project(root) {
        engines.push(EngineKind::Svelte);
    }
    if has_vue_project(root) {
        engines.push(EngineKind::Vue);
    }
    if has_assembly_project(root) {
        engines.push(EngineKind::Assembly);
    }
    if has_powershell_project(root) {
        engines.push(EngineKind::Powershell);
    }
    if has_starlark_project(root) {
        engines.push(EngineKind::Starlark);
    }
    if has_hcl_project(root) {
        engines.push(EngineKind::Hcl);
    }
    if has_typst_project(root) {
        engines.push(EngineKind::Typst);
    }
    if has_wat_project(root) {
        engines.push(EngineKind::Wat);
    }
    if has_systemverilog_project(root) {
        engines.push(EngineKind::SystemVerilog);
    }
    if has_vhdl_project(root) {
        engines.push(EngineKind::Vhdl);
    }
    if has_ballerina_project(root) {
        engines.push(EngineKind::Ballerina);
    }
    if has_jsonnet_project(root) {
        engines.push(EngineKind::Jsonnet);
    }
    if has_cue_project(root) {
        engines.push(EngineKind::Cue);
    }

    if engines.is_empty() {
        engines.push(EngineKind::Generic);
    }

    engines
}
