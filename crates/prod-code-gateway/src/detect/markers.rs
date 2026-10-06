/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

/// Manifest markers used to identify project language types.
pub const RUST_MARKERS: &[&str] = &["Cargo.toml"];
pub const GO_MARKERS: &[&str] = &["go.mod", "go.work"];
pub const PYTHON_MARKERS: &[&str] = &[
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "setup.cfg",
    "Pipfile",
];
pub const CPP_MARKERS: &[&str] = &[
    "compile_commands.json",
    "CMakeLists.txt",
    "meson.build",
    ".clangd",
];
pub const SWIFT_MARKERS: &[&str] = &["Package.swift"];
pub const TYPESCRIPT_MARKERS: &[&str] = &[
    "tsconfig.json",
    "package.json",
    "jsconfig.json",
    "deno.json",
    "deno.jsonc",
];
pub const JAVA_MARKERS: &[&str] = &[
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];
pub const CSHARP_MARKERS: &[&str] = &[
    "global.json",
    "Directory.Build.props",
    "Directory.Build.targets",
];
pub const PHP_MARKERS: &[&str] = &["composer.json"];
pub const RUBY_MARKERS: &[&str] = &["Gemfile"];
pub const DART_MARKERS: &[&str] = &["pubspec.yaml"];
pub const ZIG_MARKERS: &[&str] = &["build.zig", "build.zig.zon"];
pub const ELIXIR_MARKERS: &[&str] = &["mix.exs"];
pub const SCALA_MARKERS: &[&str] = &[
    "build.sbt",
    "build.sc",
    ".scala-build",
    "project/build.properties",
    "project/plugins.sbt",
];
pub const LUA_MARKERS: &[&str] = &[".luarc.json", ".luacheckrc"];
pub const HASKELL_MARKERS: &[&str] = &["cabal.project", "stack.yaml", "package.yaml"];
pub const OCAML_MARKERS: &[&str] = &["dune-project", "dune"];
pub const CLOJURE_MARKERS: &[&str] = &["project.clj", "deps.edn"];
pub const JULIA_MARKERS: &[&str] = &["JuliaProject.toml"];
pub const SHELL_MARKERS: &[&str] = &[".shellcheckrc"];
pub const R_MARKERS: &[&str] = &["DESCRIPTION", "NAMESPACE"];
pub const ERLANG_MARKERS: &[&str] = &["rebar.config", "rebar.lock", "erlang.mk"];
pub const PERL_MARKERS: &[&str] = &["cpanfile", "Makefile.PL", "Build.PL", "dist.ini"];
pub const SOLIDITY_MARKERS: &[&str] = &[
    "foundry.toml",
    "hardhat.config.js",
    "hardhat.config.ts",
    "hardhat.config.cjs",
    "truffle-config.js",
];
pub const NIM_MARKERS: &[&str] = &["nim.cfg"];
pub const D_MARKERS: &[&str] = &["dub.json", "dub.sdl"];
pub const FORTRAN_MARKERS: &[&str] = &["fpm.toml"];
pub const SQL_MARKERS: &[&str] = &[
    ".sqlfluff",
    "sqlfluff.cfg",
    ".sqls.json",
    "sqls.json",
    "schema.sql",
];
pub const GRAPHQL_MARKERS: &[&str] = &[
    "codegen.yml",
    "codegen.ts",
    "codegen.json",
    ".graphqlrc",
    ".graphqlrc.yml",
    ".graphqlrc.json",
    "schema.graphql",
];
pub const PROTOBUF_MARKERS: &[&str] = &[
    "buf.yaml",
    "buf.work.yaml",
    "buf.gen.yaml",
    "buf.lock",
    ".protolint.yaml",
];
pub const CRYSTAL_MARKERS: &[&str] = &["shard.yml", "shard.lock"];
pub const GROOVY_MARKERS: &[&str] = &["Jenkinsfile"];
pub const ADA_MARKERS: &[&str] = &["default.gpr"];
pub const V_MARKERS: &[&str] = &["v.mod"];
pub const RACKET_MARKERS: &[&str] = &["info.rkt"];
pub const TERRAFORM_MARKERS: &[&str] = &[
    "main.tf",
    "versions.tf",
    "terraform.tf",
    ".terraform.lock.hcl",
];
pub const NIX_MARKERS: &[&str] = &["flake.nix", "default.nix", "shell.nix", "configuration.nix"];
pub const MARKDOWN_MARKERS: &[&str] = &["README.md", ".marksman.toml"];
pub const YAML_MARKERS: &[&str] = &[
    ".yamllint",
    ".yamllint.yml",
    ".yamllint.yaml",
    ".gitlab-ci.yml",
    "docker-compose.yml",
    "docker-compose.yaml",
    "compose.yaml",
    "compose.yml",
];
pub const TOML_MARKERS: &[&str] = &["taplo.toml", ".taplo.toml"];
pub const JSON_MARKERS: &[&str] = &[".jsonlintrc", ".jsonlintrc.json", ".jsonlint"];
pub const HTML_MARKERS: &[&str] = &["index.html", "htmlhint.json", ".htmlhintrc"];
pub const CSS_MARKERS: &[&str] = &[
    "stylelint.config.js",
    "stylelint.config.cjs",
    "stylelint.config.mjs",
    ".stylelintrc",
    ".stylelintrc.json",
    ".stylelintrc.yml",
    "styles.css",
];
pub const DOCKERFILE_MARKERS: &[&str] = &[
    "Dockerfile",
    "Containerfile",
    ".hadolint.yaml",
    ".hadolint.yml",
];
pub const SVELTE_MARKERS: &[&str] = &["svelte.config.js", "svelte.config.ts"];
pub const VUE_MARKERS: &[&str] = &["vue.config.js", "vue.config.ts"];
pub const ASSEMBLY_MARKERS: &[&str] = &[".asm-lsp.toml"];
pub const POWERSHELL_MARKERS: &[&str] = &["PSScriptAnalyzerSettings.psd1", "profile.ps1"];
pub const STARLARK_MARKERS: &[&str] = &[
    "BUILD.bazel",
    "WORKSPACE.bazel",
    "MODULE.bazel",
    "BUILD",
    "WORKSPACE",
    "Tiltfile",
];
pub const HCL_MARKERS: &[&str] = &["terragrunt.hcl", ".tflint.hcl", "packer.pkr.hcl"];
pub const TYPST_MARKERS: &[&str] = &["typst.toml"];
pub const WAT_MARKERS: &[&str] = &["wat.json"];
pub const SYSTEMVERILOG_MARKERS: &[&str] = &["verilator.f", "filelist.f"];
pub const VHDL_MARKERS: &[&str] = &["vunit.py", "ghdl.flags"];
pub const BALLERINA_MARKERS: &[&str] = &["Ballerina.toml", "Dependencies.toml"];
pub const JSONNET_MARKERS: &[&str] = &["jsonnetfile.json", "jsonnetfile.lock.json"];
pub const CUE_MARKERS: &[&str] = &["cue.mod"];

/// The names a Makefile goes by.
pub const MAKEFILES: &[&str] = &["Makefile", "makefile", "GNUmakefile"];

/// The extensions of C and C++ sources and headers.
pub const C_SOURCE_EXTENSIONS: &[&str] = &["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"];
