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
use std::str::FromStr;

use super::heuristics::*;
use super::markers::*;

/// Detect the primary engine kind for the specified workspace path.
///
/// Priority order:
/// 1. Rust (`Cargo.toml`)
/// 2. Go (`go.mod`, `go.work`)
/// 3. Swift (`Package.swift`, an XcodeGen `project.yml`, an Xcode bundle)
/// 4. C/C++ (`compile_commands.json`, `CMakeLists.txt`, `meson.build`, `.clangd`)
/// 5. Python (`pyproject.toml`, `requirements.txt`, `setup.py`, etc.)
/// Structured and language-specific project markers take priority over generic Markdown files.
pub fn detect_engine(root: &Path) -> EngineKind {
    for marker in RUST_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Rust;
        }
    }
    for marker in GO_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Go;
        }
    }
    if has_swift_project(root) {
        return EngineKind::Swift;
    }
    for marker in CPP_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Cpp;
        }
    }
    for marker in PYTHON_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Python;
        }
    }
    if has_csharp_project(root) {
        return EngineKind::Csharp;
    }
    if has_kotlin_project(root) {
        return EngineKind::Kotlin;
    }
    if SCALA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return EngineKind::Scala;
    }
    if has_groovy_gradle_sources(root) {
        return EngineKind::Groovy;
    }
    if has_java_project(root) {
        return EngineKind::Java;
    }
    if has_svelte_project(root) {
        return EngineKind::Svelte;
    }
    if has_vue_project(root) {
        return EngineKind::Vue;
    }
    for marker in TYPESCRIPT_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::TypeScript;
        }
    }
    if is_make_cpp_project(root) {
        return EngineKind::Cpp;
    }
    for marker in PHP_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Php;
        }
    }
    for marker in RUBY_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Ruby;
        }
    }
    if DART_MARKERS.iter().any(|m| root.join(m).exists()) {
        return EngineKind::Dart;
    }
    if ZIG_MARKERS.iter().any(|m| root.join(m).exists()) {
        return EngineKind::Zig;
    }
    if ELIXIR_MARKERS.iter().any(|m| root.join(m).exists()) {
        return EngineKind::Elixir;
    }
    if has_lua_project(root) {
        return EngineKind::Lua;
    }
    if has_haskell_project(root) {
        return EngineKind::Haskell;
    }
    if has_ocaml_project(root) {
        return EngineKind::Ocaml;
    }
    if has_clojure_project(root) {
        return EngineKind::Clojure;
    }
    if has_julia_project(root) {
        return EngineKind::Julia;
    }
    if has_r_project(root) {
        return EngineKind::R;
    }
    if has_erlang_project(root) {
        return EngineKind::Erlang;
    }
    if has_fsharp_project(root) {
        return EngineKind::Fsharp;
    }
    if has_perl_project(root) {
        return EngineKind::Perl;
    }
    if has_solidity_project(root) {
        return EngineKind::Solidity;
    }
    if has_nim_project(root) {
        return EngineKind::Nim;
    }
    if has_d_project(root) {
        return EngineKind::D;
    }
    if has_fortran_project(root) {
        return EngineKind::Fortran;
    }
    if has_sql_project(root) {
        return EngineKind::Sql;
    }
    if has_graphql_project(root) {
        return EngineKind::Graphql;
    }
    if has_protobuf_project(root) {
        return EngineKind::Protobuf;
    }
    if has_crystal_project(root) {
        return EngineKind::Crystal;
    }
    if has_groovy_project(root) {
        return EngineKind::Groovy;
    }
    if has_ada_project(root) {
        return EngineKind::Ada;
    }
    if has_v_project(root) {
        return EngineKind::V;
    }
    if has_racket_project(root) {
        return EngineKind::Racket;
    }
    if has_terraform_project(root) {
        return EngineKind::Terraform;
    }
    if has_nix_project(root) {
        return EngineKind::Nix;
    }
    if has_assembly_project(root) {
        return EngineKind::Assembly;
    }
    if has_dockerfile_project(root) {
        return EngineKind::Dockerfile;
    }
    if has_yaml_project(root) {
        return EngineKind::Yaml;
    }
    if has_toml_project(root) {
        return EngineKind::Toml;
    }
    if has_json_project(root) {
        return EngineKind::Json;
    }
    if has_html_project(root) {
        return EngineKind::Html;
    }
    if has_css_project(root) {
        return EngineKind::Css;
    }
    if has_powershell_project(root) {
        return EngineKind::Powershell;
    }
    if has_starlark_project(root) {
        return EngineKind::Starlark;
    }
    if has_hcl_project(root) {
        return EngineKind::Hcl;
    }
    if has_typst_project(root) {
        return EngineKind::Typst;
    }
    if has_wat_project(root) {
        return EngineKind::Wat;
    }
    if has_systemverilog_project(root) {
        return EngineKind::SystemVerilog;
    }
    if has_vhdl_project(root) {
        return EngineKind::Vhdl;
    }
    if has_ballerina_project(root) {
        return EngineKind::Ballerina;
    }
    if has_jsonnet_project(root) {
        return EngineKind::Jsonnet;
    }
    if has_cue_project(root) {
        return EngineKind::Cue;
    }
    if has_shell_project(root) {
        return EngineKind::Shell;
    }
    if has_markdown_project(root) {
        return EngineKind::Markdown;
    }
    EngineKind::Generic
}

/// Resolve the effective engine, honoring an explicit client preference if provided.
pub fn resolve_engine(root: &Path, preferred: Option<&str>) -> EngineKind {
    if let Some(pref) = preferred.filter(|p| !p.trim().is_empty()) {
        return EngineKind::from_str(pref.trim()).unwrap_or(EngineKind::Generic);
    }
    detect_engine(root)
}
