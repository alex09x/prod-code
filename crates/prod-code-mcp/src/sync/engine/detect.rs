/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::engine::workspace::{has_c_sources, mcp_has_kotlin_project};
use std::path::Path;

/// The engine a directory's own manifests ask for. A Makefile next to C or C++ sources, at the
/// root or in `src/`, is a C/C++ project built with Make, below every other manifest: Go,
/// Python and JavaScript repositories keep a Makefile of tasks too. A `project.yml` with
/// top-level `targets:` is an XcodeGen spec, a Swift project whose Xcode project is generated
/// (#404). The gateway's `detect_engine` decides the same way.
pub(crate) fn engine_at(root: &Path) -> Option<&'static str> {
    let has = |name: &str| root.join(name).exists();
    let has_ext = |extensions: &[&str]| {
        std::fs::read_dir(root).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        extensions.contains(&extension.to_ascii_lowercase().as_str())
                    })
            })
        })
    };
    let has_prefix = |prefix: &str| {
        std::fs::read_dir(root).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().starts_with(prefix))
        })
    };
    let has_xcode = std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let n = e.file_name();
                let n = n.to_string_lossy();
                n.ends_with(".xcodeproj") || n.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false);
    let xcodegen = std::fs::read_to_string(root.join("project.yml"))
        .is_ok_and(|text| text.lines().any(|line| line.starts_with("targets:")));
    let make_cpp = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .any(|m| root.join(m).is_file())
        && (has_c_sources(root) || has_c_sources(&root.join("src")));
    let has_csharp = has("global.json")
        || has("Directory.Build.props")
        || has("Directory.Build.targets")
        || std::fs::read_dir(root)
            .map(|entries| {
                entries.flatten().any(|e| {
                    let n = e.file_name();
                    let n = n.to_string_lossy();
                    n.ends_with(".csproj") || n.ends_with(".sln")
                })
            })
            .unwrap_or(false);
    let has_kotlin = mcp_has_kotlin_project(root);
    let has_scala = has("build.sbt")
        || has("build.sc")
        || has(".scala-build")
        || root.join("project/build.properties").exists()
        || root.join("project/plugins.sbt").exists();
    let has_groovy_gradle_sources = has("build.gradle") && root.join("src/main/groovy").is_dir();
    if has("Cargo.toml") {
        Some("rust")
    } else if has("go.mod") || has("go.work") {
        Some("go")
    } else if has("Package.swift") || has_xcode || xcodegen {
        Some("swift")
    } else if has("compile_commands.json")
        || has("CMakeLists.txt")
        || has("meson.build")
        || has(".clangd")
    {
        Some("cpp")
    } else if has("pyproject.toml")
        || has("requirements.txt")
        || has("setup.py")
        || has("setup.cfg")
        || has("Pipfile")
    {
        Some("python")
    } else if has_csharp {
        Some("csharp")
    } else if has_kotlin {
        Some("kotlin")
    } else if has_scala {
        Some("scala")
    } else if has_groovy_gradle_sources {
        Some("groovy")
    } else if has("pom.xml")
        || has("build.gradle")
        || has("build.gradle.kts")
        || has("settings.gradle")
        || has("settings.gradle.kts")
    {
        Some("java")
    } else if has("svelte.config.js") || has("svelte.config.ts") || has_ext(&["svelte"]) {
        Some("svelte")
    } else if has("vue.config.js") || has("vue.config.ts") || has_ext(&["vue"]) {
        Some("vue")
    } else if has("tsconfig.json")
        || has("package.json")
        || has("jsconfig.json")
        || has("deno.json")
        || has("deno.jsonc")
    {
        Some("typescript")
    } else if make_cpp {
        Some("cpp")
    } else if has("composer.json") {
        Some("php")
    } else if has("Gemfile") {
        Some("ruby")
    } else if has("pubspec.yaml") {
        Some("dart")
    } else if has("build.zig") || has("build.zig.zon") {
        Some("zig")
    } else if has("mix.exs") {
        Some("elixir")
    } else if has(".luarc.json") || has(".luacheckrc") || has_ext(&["lua"]) {
        Some("lua")
    } else if has("cabal.project")
        || has("stack.yaml")
        || has("package.yaml")
        || has_ext(&["hs", "lhs", "cabal"])
    {
        Some("haskell")
    } else if has("dune-project") || has("dune") || has_ext(&["ml", "mli"]) {
        Some("ocaml")
    } else if has("project.clj") || has("deps.edn") || has_ext(&["clj", "cljs", "cljc"]) {
        Some("clojure")
    } else if has("JuliaProject.toml") || has_ext(&["jl"]) {
        Some("julia")
    } else if has("DESCRIPTION") || has("NAMESPACE") || has_ext(&["r"]) {
        Some("r")
    } else if has("rebar.config") || has("rebar.lock") || has("erlang.mk") || has_ext(&["erl"]) {
        Some("erlang")
    } else if has_ext(&["fs", "fsi", "fsx", "fsproj"]) {
        Some("fsharp")
    } else if has("cpanfile")
        || has("Makefile.PL")
        || has("Build.PL")
        || has("dist.ini")
        || has_ext(&["pl", "pm"])
    {
        Some("perl")
    } else if has("foundry.toml")
        || has("hardhat.config.js")
        || has("hardhat.config.ts")
        || has("hardhat.config.cjs")
        || has("truffle-config.js")
        || has_ext(&["sol"])
    {
        Some("solidity")
    } else if has("nim.cfg") || has_ext(&["nimble"]) {
        Some("nim")
    } else if has("dub.json") || has("dub.sdl") {
        Some("d")
    } else if has("fpm.toml") || has_ext(&["f", "for", "f90", "f95", "f03", "f08"]) {
        Some("fortran")
    } else if has(".sqlfluff")
        || has("sqlfluff.cfg")
        || has(".sqls.json")
        || has("sqls.json")
        || has("schema.sql")
        || has_ext(&["sql"])
    {
        Some("sql")
    } else if has("codegen.yml")
        || has("codegen.ts")
        || has("codegen.json")
        || has(".graphqlrc")
        || has(".graphqlrc.yml")
        || has(".graphqlrc.json")
        || has("schema.graphql")
        || has_ext(&["graphql", "gql"])
    {
        Some("graphql")
    } else if has("buf.yaml")
        || has("buf.work.yaml")
        || has("buf.gen.yaml")
        || has("buf.lock")
        || has(".protolint.yaml")
        || has_ext(&["proto"])
    {
        Some("protobuf")
    } else if has("shard.yml") || has("shard.lock") || has_ext(&["cr"]) {
        Some("crystal")
    } else if has("Jenkinsfile") || has_ext(&["groovy", "gvy"]) {
        Some("groovy")
    } else if has("default.gpr") || has_ext(&["gpr", "adb", "ads"]) {
        Some("ada")
    } else if has("v.mod") || has_ext(&["vsh"]) {
        Some("v")
    } else if has("info.rkt") || has_ext(&["rkt"]) {
        Some("racket")
    } else if has("main.tf")
        || has("versions.tf")
        || has("terraform.tf")
        || has(".terraform.lock.hcl")
        || has_ext(&["tf", "tofu"])
    {
        Some("terraform")
    } else if has("flake.nix")
        || has("default.nix")
        || has("shell.nix")
        || has("configuration.nix")
        || has_ext(&["nix"])
    {
        Some("nix")
    } else if has(".asm-lsp.toml") || has_ext(&["asm", "nasm", "s"]) {
        Some("assembly")
    } else if has_prefix("Dockerfile")
        || has_prefix("Containerfile")
        || has(".hadolint.yaml")
        || has(".hadolint.yml")
        || has_ext(&["dockerfile"])
    {
        Some("dockerfile")
    } else if has("PSScriptAnalyzerSettings.psd1")
        || has("profile.ps1")
        || has_ext(&["ps1", "psm1", "psd1"])
    {
        Some("powershell")
    } else if has("BUILD.bazel")
        || has("WORKSPACE.bazel")
        || has("MODULE.bazel")
        || has("BUILD")
        || has("WORKSPACE")
        || has("Tiltfile")
        || has_ext(&["bzl", "star"])
    {
        Some("starlark")
    } else if has("terragrunt.hcl") || has(".tflint.hcl") || has_ext(&["hcl"]) {
        Some("hcl")
    } else if has("typst.toml") || has_ext(&["typ"]) {
        Some("typst")
    } else if has("wat.json") || has_ext(&["wat", "wast"]) {
        Some("wat")
    } else if has("verilator.f") || has_ext(&["sv", "svh"]) {
        Some("systemverilog")
    } else if has("vunit.py") || has_ext(&["vhd", "vhdl"]) {
        Some("vhdl")
    } else if has("Ballerina.toml") || has_ext(&["bal"]) {
        Some("ballerina")
    } else if has("jsonnetfile.json") || has_ext(&["jsonnet", "libsonnet"]) {
        Some("jsonnet")
    } else if has("cue.mod") || has_ext(&["cue"]) {
        Some("cue")
    } else if has(".yamllint")
        || has(".yamllint.yml")
        || has(".yamllint.yaml")
        || has(".gitlab-ci.yml")
        || has("docker-compose.yml")
        || has("docker-compose.yaml")
        || has("compose.yaml")
        || has("compose.yml")
    {
        Some("yaml")
    } else if has("taplo.toml") || has(".taplo.toml") {
        Some("toml")
    } else if has(".jsonlintrc") || has(".jsonlintrc.json") || has(".jsonlint") {
        Some("json")
    } else if has("index.html")
        || has("htmlhint.json")
        || has(".htmlhintrc")
        || has_ext(&["html", "htm"])
    {
        Some("html")
    } else if has("stylelint.config.js")
        || has("stylelint.config.cjs")
        || has("stylelint.config.mjs")
        || has(".stylelintrc")
        || has(".stylelintrc.json")
        || has(".stylelintrc.yml")
        || has("styles.css")
        || has_ext(&["css", "scss", "less"])
    {
        Some("css")
    } else if has(".shellcheckrc") || has_ext(&["sh", "bash", "zsh"]) {
        Some("shell")
    } else if has(".marksman.toml") {
        Some("markdown")
    } else {
        None
    }
}
