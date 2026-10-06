/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use prod_code_protocol::messages::EngineKind;
use tempfile::tempdir;

#[test]
fn test_universal_language_detection_matrix_all_60_languages() {
    struct Case {
        lang: &'static str,
        files: &'static [(&'static str, &'static str)],
        kind: EngineKind,
    }

    let matrix = [
        Case {
            lang: "Rust",
            files: &[("Cargo.toml", "[workspace]")],
            kind: EngineKind::Rust,
        },
        Case {
            lang: "Go",
            files: &[("go.mod", "module test")],
            kind: EngineKind::Go,
        },
        Case {
            lang: "Python",
            files: &[("pyproject.toml", "[project]")],
            kind: EngineKind::Python,
        },
        Case {
            lang: "TypeScript",
            files: &[("tsconfig.json", "{}")],
            kind: EngineKind::TypeScript,
        },
        Case {
            lang: "Cpp",
            files: &[("CMakeLists.txt", "project(test)")],
            kind: EngineKind::Cpp,
        },
        Case {
            lang: "Swift",
            files: &[("Package.swift", "// swift-tools-version:5.9")],
            kind: EngineKind::Swift,
        },
        Case {
            lang: "Java",
            files: &[("pom.xml", "<project></project>")],
            kind: EngineKind::Java,
        },
        Case {
            lang: "Kotlin",
            files: &[(
                "build.gradle.kts",
                "plugins { kotlin(\"jvm\") version \"2.0.0\" }",
            )],
            kind: EngineKind::Kotlin,
        },
        Case {
            lang: "Csharp",
            files: &[("global.json", "{}")],
            kind: EngineKind::Csharp,
        },
        Case {
            lang: "Php",
            files: &[("composer.json", "{}")],
            kind: EngineKind::Php,
        },
        Case {
            lang: "Ruby",
            files: &[("Gemfile", "")],
            kind: EngineKind::Ruby,
        },
        Case {
            lang: "Dart",
            files: &[("pubspec.yaml", "name: test")],
            kind: EngineKind::Dart,
        },
        Case {
            lang: "Zig",
            files: &[("build.zig", "")],
            kind: EngineKind::Zig,
        },
        Case {
            lang: "Elixir",
            files: &[("mix.exs", "defmodule M do end")],
            kind: EngineKind::Elixir,
        },
        Case {
            lang: "Scala",
            files: &[("build.sbt", "")],
            kind: EngineKind::Scala,
        },
        Case {
            lang: "Lua",
            files: &[(".luarc.json", "{}")],
            kind: EngineKind::Lua,
        },
        Case {
            lang: "Haskell",
            files: &[("cabal.project", "")],
            kind: EngineKind::Haskell,
        },
        Case {
            lang: "Ocaml",
            files: &[("dune-project", "(lang dune 3.0)")],
            kind: EngineKind::Ocaml,
        },
        Case {
            lang: "Clojure",
            files: &[("project.clj", "")],
            kind: EngineKind::Clojure,
        },
        Case {
            lang: "Julia",
            files: &[("JuliaProject.toml", "")],
            kind: EngineKind::Julia,
        },
        Case {
            lang: "Shell",
            files: &[(".shellcheckrc", "")],
            kind: EngineKind::Shell,
        },
        Case {
            lang: "R",
            files: &[("DESCRIPTION", "Package: test")],
            kind: EngineKind::R,
        },
        Case {
            lang: "Erlang",
            files: &[("rebar.config", "")],
            kind: EngineKind::Erlang,
        },
        Case {
            lang: "Fsharp",
            files: &[("App.fsproj", "")],
            kind: EngineKind::Fsharp,
        },
        Case {
            lang: "Perl",
            files: &[("cpanfile", "")],
            kind: EngineKind::Perl,
        },
        Case {
            lang: "Solidity",
            files: &[("foundry.toml", "")],
            kind: EngineKind::Solidity,
        },
        Case {
            lang: "Nim",
            files: &[("nim.cfg", "")],
            kind: EngineKind::Nim,
        },
        Case {
            lang: "D",
            files: &[("dub.json", "{}")],
            kind: EngineKind::D,
        },
        Case {
            lang: "Fortran",
            files: &[("fpm.toml", "")],
            kind: EngineKind::Fortran,
        },
        Case {
            lang: "Sql",
            files: &[(".sqlfluff", "")],
            kind: EngineKind::Sql,
        },
        Case {
            lang: "Graphql",
            files: &[("codegen.yml", "")],
            kind: EngineKind::Graphql,
        },
        Case {
            lang: "Protobuf",
            files: &[("buf.yaml", "")],
            kind: EngineKind::Protobuf,
        },
        Case {
            lang: "Crystal",
            files: &[("shard.yml", "")],
            kind: EngineKind::Crystal,
        },
        Case {
            lang: "Groovy",
            files: &[("Jenkinsfile", "")],
            kind: EngineKind::Groovy,
        },
        Case {
            lang: "Ada",
            files: &[("default.gpr", "")],
            kind: EngineKind::Ada,
        },
        Case {
            lang: "V",
            files: &[("v.mod", "")],
            kind: EngineKind::V,
        },
        Case {
            lang: "Racket",
            files: &[("info.rkt", "")],
            kind: EngineKind::Racket,
        },
        Case {
            lang: "Terraform",
            files: &[("main.tf", "terraform {}")],
            kind: EngineKind::Terraform,
        },
        Case {
            lang: "Nix",
            files: &[("flake.nix", "{ description = \"test\"; }")],
            kind: EngineKind::Nix,
        },
        Case {
            lang: "Markdown",
            files: &[("README.md", "# Test")],
            kind: EngineKind::Markdown,
        },
        Case {
            lang: "Yaml",
            files: &[(".yamllint", "extends: default")],
            kind: EngineKind::Yaml,
        },
        Case {
            lang: "Toml",
            files: &[("taplo.toml", "")],
            kind: EngineKind::Toml,
        },
        Case {
            lang: "Json",
            files: &[(".jsonlintrc", "{}")],
            kind: EngineKind::Json,
        },
        Case {
            lang: "Html",
            files: &[("index.html", "<!doctype html>")],
            kind: EngineKind::Html,
        },
        Case {
            lang: "Css",
            files: &[("styles.css", "body {}")],
            kind: EngineKind::Css,
        },
        Case {
            lang: "Dockerfile",
            files: &[("Dockerfile", "FROM alpine")],
            kind: EngineKind::Dockerfile,
        },
        Case {
            lang: "Svelte",
            files: &[("svelte.config.js", "export default {};")],
            kind: EngineKind::Svelte,
        },
        Case {
            lang: "Vue",
            files: &[("vue.config.js", "module.exports = {};")],
            kind: EngineKind::Vue,
        },
        Case {
            lang: "Assembly",
            files: &[(".asm-lsp.toml", "")],
            kind: EngineKind::Assembly,
        },
        Case {
            lang: "Powershell",
            files: &[("scripts.ps1", "Write-Output 'hi'")],
            kind: EngineKind::Powershell,
        },
        Case {
            lang: "Starlark",
            files: &[("BUILD.bazel", "load(':rules.bzl', 'rule')")],
            kind: EngineKind::Starlark,
        },
        Case {
            lang: "Hcl",
            files: &[(
                "terragrunt.hcl",
                "include { path = find_in_parent_folders() }",
            )],
            kind: EngineKind::Hcl,
        },
        Case {
            lang: "Typst",
            files: &[("paper.typ", "#set text(font: 'PT Serif')")],
            kind: EngineKind::Typst,
        },
        Case {
            lang: "Wat",
            files: &[("main.wat", "(module (func (export \"run\")))")],
            kind: EngineKind::Wat,
        },
        Case {
            lang: "SystemVerilog",
            files: &[("core.sv", "module cpu; endmodule")],
            kind: EngineKind::SystemVerilog,
        },
        Case {
            lang: "Vhdl",
            files: &[("alu.vhd", "entity alu is end entity;")],
            kind: EngineKind::Vhdl,
        },
        Case {
            lang: "Ballerina",
            files: &[("Ballerina.toml", "[package]\nname = \"demo\"")],
            kind: EngineKind::Ballerina,
        },
        Case {
            lang: "Jsonnet",
            files: &[("service.jsonnet", "local config = {}; config")],
            kind: EngineKind::Jsonnet,
        },
        Case {
            lang: "Cue",
            files: &[("cue.mod", "module: \"example.com\"")],
            kind: EngineKind::Cue,
        },
        Case {
            lang: "Generic",
            files: &[],
            kind: EngineKind::Generic,
        },
    ];

    for case in matrix {
        let dir = tempdir().unwrap();
        for (filename, content) in case.files {
            std::fs::write(dir.path().join(filename), content).unwrap();
        }
        let detected = detect_engine(dir.path());
        assert_eq!(
            detected, case.kind,
            "Language {} detection mismatch: expected {:?}, got {:?}",
            case.lang, case.kind, detected
        );
        let all = detect_all_engines(dir.path());
        assert!(
            all.contains(&case.kind),
            "Language {} missing from detect_all_engines: {all:?}",
            case.lang
        );
    }
}
