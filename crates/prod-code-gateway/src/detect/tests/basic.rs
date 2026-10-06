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
fn test_detect_cpp_and_swift_workspaces() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("CMakeLists.txt"), "project(x)").unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::Cpp);
    std::fs::write(
        dir.path().join("Package.swift"),
        "// swift-tools-version:5.9",
    )
    .unwrap();
    assert_eq!(
        detect_engine(dir.path()),
        EngineKind::Swift,
        "Swift outranks C++"
    );
    let xc = tempdir().unwrap();
    std::fs::create_dir_all(xc.path().join("App.xcodeproj")).unwrap();
    assert_eq!(detect_engine(xc.path()), EngineKind::Swift);
    std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();
    assert_eq!(
        detect_engine(dir.path()),
        EngineKind::Rust,
        "Rust outranks Swift"
    );
    assert!(detect_all_engines(dir.path()).contains(&EngineKind::Cpp));
}

/// A Makefile next to C sources is C/C++, below every other manifest; one with no C sources
/// is not; an XcodeGen `project.yml` is Swift, another `project.yml` is not (#404).
#[test]
fn a_make_c_project_and_an_xcodegen_spec_are_detected() {
    let make = tempdir().unwrap();
    std::fs::write(make.path().join("Makefile"), "all:\n\tcc main.c\n").unwrap();
    assert_eq!(
        detect_engine(make.path()),
        EngineKind::Generic,
        "no C sources"
    );
    std::fs::create_dir_all(make.path().join("src")).unwrap();
    std::fs::write(make.path().join("src/main.c"), "int main(void){}\n").unwrap();
    assert_eq!(detect_engine(make.path()), EngineKind::Cpp);
    assert_eq!(detect_all_engines(make.path()), vec![EngineKind::Cpp]);
    std::fs::write(make.path().join("package.json"), "{}").unwrap();
    assert_eq!(
        detect_engine(make.path()),
        EngineKind::TypeScript,
        "a Makefile ranks below every manifest"
    );
    let go = tempdir().unwrap();
    std::fs::write(go.path().join("go.mod"), "module m\n").unwrap();
    std::fs::write(go.path().join("Makefile"), "test:\n\tgo test ./...\n").unwrap();
    std::fs::write(go.path().join("cgo.h"), "").unwrap();
    assert_eq!(detect_engine(go.path()), EngineKind::Go);

    let xcodegen = tempdir().unwrap();
    std::fs::write(
        xcodegen.path().join("project.yml"),
        "name: App\ntargets:\n  App:\n    type: application\n",
    )
    .unwrap();
    assert_eq!(detect_engine(xcodegen.path()), EngineKind::Swift);
    let other = tempdir().unwrap();
    std::fs::write(other.path().join("project.yml"), "name: docs\npages: []\n").unwrap();
    assert_eq!(detect_engine(other.path()), EngineKind::Generic);
}

#[test]
fn test_detect_rust_workspace() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::Rust);
    assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Rust]);
}

#[test]
fn test_detect_go_workspace() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("go.mod"),
        "module example.com/test\n\ngo 1.22",
    )
    .unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::Go);
    assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Go]);
}

#[test]
fn test_detect_python_workspace() {
    let dir = tempdir().unwrap();
    std::fs::write(
        dir.path().join("pyproject.toml"),
        "[project]\nname = \"test\"",
    )
    .unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::Python);

    let dir2 = tempdir().unwrap();
    std::fs::write(dir2.path().join("requirements.txt"), "requests>=2.0").unwrap();
    assert_eq!(detect_engine(dir2.path()), EngineKind::Python);
}

#[test]
fn test_detect_typescript_workspace() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("package.json"), "{\"name\": \"test\"}").unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::TypeScript);

    let dir2 = tempdir().unwrap();
    std::fs::write(dir2.path().join("tsconfig.json"), "{}").unwrap();
    assert_eq!(detect_engine(dir2.path()), EngineKind::TypeScript);
}

#[test]
fn test_detect_empty_fallback() {
    let dir = tempdir().unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::Generic);
    assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Generic]);
}

#[test]
fn test_polyglot_monorepo() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[workspace]").unwrap();
    std::fs::write(dir.path().join("package.json"), "{}").unwrap();
    std::fs::write(dir.path().join("go.mod"), "module test").unwrap();

    // Primary follows priority (Rust > Go > Python > TS)
    assert_eq!(detect_engine(dir.path()), EngineKind::Rust);

    // All detected engines returns all 3
    let all = detect_all_engines(dir.path());
    assert_eq!(
        all,
        vec![EngineKind::Rust, EngineKind::Go, EngineKind::TypeScript,]
    );
}

#[test]
fn test_preferred_engine_override() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[workspace]").unwrap();

    // With no preference, detects Rust
    assert_eq!(resolve_engine(dir.path(), None), EngineKind::Rust);

    // With preference for Go, overrides to Go
    assert_eq!(resolve_engine(dir.path(), Some("go")), EngineKind::Go);
    assert_eq!(
        resolve_engine(dir.path(), Some("python")),
        EngineKind::Python
    );
}

#[test]
fn test_detect_additional_languages() {
    let dir = tempdir().unwrap();
    std::fs::write(dir.path().join("pom.xml"), "<project></project>").unwrap();
    assert_eq!(detect_engine(dir.path()), EngineKind::Java);
    assert_eq!(detect_all_engines(dir.path()), vec![EngineKind::Java]);

    let dir_kt = tempdir().unwrap();
    std::fs::write(
        dir_kt.path().join("build.gradle.kts"),
        "plugins { kotlin(\"jvm\") version \"2.0.0\" }",
    )
    .unwrap();
    std::fs::create_dir_all(dir_kt.path().join("src/main/kotlin")).unwrap();
    std::fs::write(
        dir_kt.path().join("src/main/kotlin/Main.kt"),
        "fun main() {}",
    )
    .unwrap();
    assert_eq!(detect_engine(dir_kt.path()), EngineKind::Kotlin);
    assert_eq!(detect_all_engines(dir_kt.path()), vec![EngineKind::Kotlin]);

    let dir_java_kts = tempdir().unwrap();
    std::fs::write(
        dir_java_kts.path().join("build.gradle.kts"),
        "plugins { java }\ndependencies {\n    implementation(\"org.jetbrains.kotlin:kotlin-stdlib:1.9.0\")\n}",
    )
    .unwrap();
    std::fs::create_dir_all(dir_java_kts.path().join("src/main/java")).unwrap();
    std::fs::write(
        dir_java_kts.path().join("src/main/java/Main.java"),
        "class Main {}",
    )
    .unwrap();
    assert_eq!(detect_engine(dir_java_kts.path()), EngineKind::Java);
    assert_eq!(
        detect_all_engines(dir_java_kts.path()),
        vec![EngineKind::Java]
    );

    let dir_groovy_gradle = tempdir().unwrap();
    std::fs::write(dir_groovy_gradle.path().join("build.gradle"), "").unwrap();
    std::fs::create_dir_all(dir_groovy_gradle.path().join("src/main/groovy")).unwrap();
    assert_eq!(detect_engine(dir_groovy_gradle.path()), EngineKind::Groovy);
    assert!(detect_all_engines(dir_groovy_gradle.path()).contains(&EngineKind::Groovy));

    let dir_cs = tempdir().unwrap();
    std::fs::write(dir_cs.path().join("App.csproj"), "<Project></Project>").unwrap();
    assert_eq!(detect_engine(dir_cs.path()), EngineKind::Csharp);
    assert_eq!(detect_all_engines(dir_cs.path()), vec![EngineKind::Csharp]);

    let dir_php = tempdir().unwrap();
    std::fs::write(dir_php.path().join("composer.json"), "{}").unwrap();
    assert_eq!(detect_engine(dir_php.path()), EngineKind::Php);
    assert_eq!(detect_all_engines(dir_php.path()), vec![EngineKind::Php]);

    let dir_rb = tempdir().unwrap();
    std::fs::write(
        dir_rb.path().join("Gemfile"),
        "source 'https://rubygems.org'",
    )
    .unwrap();
    assert_eq!(detect_engine(dir_rb.path()), EngineKind::Ruby);
    assert_eq!(detect_all_engines(dir_rb.path()), vec![EngineKind::Ruby]);

    let dir_dart = tempdir().unwrap();
    std::fs::write(dir_dart.path().join("pubspec.yaml"), "name: test").unwrap();
    assert_eq!(detect_engine(dir_dart.path()), EngineKind::Dart);
    assert_eq!(detect_all_engines(dir_dart.path()), vec![EngineKind::Dart]);

    let dir_zig = tempdir().unwrap();
    std::fs::write(dir_zig.path().join("build.zig"), "").unwrap();
    assert_eq!(detect_engine(dir_zig.path()), EngineKind::Zig);
    assert_eq!(detect_all_engines(dir_zig.path()), vec![EngineKind::Zig]);

    let dir_ex = tempdir().unwrap();
    std::fs::write(dir_ex.path().join("mix.exs"), "defmodule Test do end").unwrap();
    assert_eq!(detect_engine(dir_ex.path()), EngineKind::Elixir);
    assert_eq!(detect_all_engines(dir_ex.path()), vec![EngineKind::Elixir]);

    let dir_scala = tempdir().unwrap();
    std::fs::write(dir_scala.path().join("build.sbt"), "name := \"test\"").unwrap();
    assert_eq!(detect_engine(dir_scala.path()), EngineKind::Scala);
    assert_eq!(
        detect_all_engines(dir_scala.path()),
        vec![EngineKind::Scala]
    );

    let dir_lua = tempdir().unwrap();
    std::fs::write(dir_lua.path().join(".luarc.json"), "{}").unwrap();
    assert_eq!(detect_engine(dir_lua.path()), EngineKind::Lua);
    assert_eq!(detect_all_engines(dir_lua.path()), vec![EngineKind::Lua]);

    let dir_hs = tempdir().unwrap();
    std::fs::write(dir_hs.path().join("cabal.project"), "packages: .").unwrap();
    assert_eq!(detect_engine(dir_hs.path()), EngineKind::Haskell);
    assert_eq!(detect_all_engines(dir_hs.path()), vec![EngineKind::Haskell]);

    let dir_ml = tempdir().unwrap();
    std::fs::write(dir_ml.path().join("dune-project"), "(lang dune 3.0)").unwrap();
    assert_eq!(detect_engine(dir_ml.path()), EngineKind::Ocaml);
    assert_eq!(detect_all_engines(dir_ml.path()), vec![EngineKind::Ocaml]);

    let dir_clj = tempdir().unwrap();
    std::fs::write(dir_clj.path().join("project.clj"), "(defproject p \"0.1\")").unwrap();
    assert_eq!(detect_engine(dir_clj.path()), EngineKind::Clojure);
    assert_eq!(
        detect_all_engines(dir_clj.path()),
        vec![EngineKind::Clojure]
    );

    let dir_jl = tempdir().unwrap();
    std::fs::write(dir_jl.path().join("JuliaProject.toml"), "name = \"Pkg\"").unwrap();
    assert_eq!(detect_engine(dir_jl.path()), EngineKind::Julia);
    assert_eq!(detect_all_engines(dir_jl.path()), vec![EngineKind::Julia]);

    let dir_sh = tempdir().unwrap();
    std::fs::write(dir_sh.path().join(".shellcheckrc"), "").unwrap();
    assert_eq!(detect_engine(dir_sh.path()), EngineKind::Shell);
    assert_eq!(detect_all_engines(dir_sh.path()), vec![EngineKind::Shell]);

    let dir_r = tempdir().unwrap();
    std::fs::write(dir_r.path().join("DESCRIPTION"), "Package: pkg").unwrap();
    assert_eq!(detect_engine(dir_r.path()), EngineKind::R);
    assert_eq!(detect_all_engines(dir_r.path()), vec![EngineKind::R]);

    let dir_erl = tempdir().unwrap();
    std::fs::write(dir_erl.path().join("rebar.config"), "{erl_opts, []}.").unwrap();
    assert_eq!(detect_engine(dir_erl.path()), EngineKind::Erlang);
    assert_eq!(detect_all_engines(dir_erl.path()), vec![EngineKind::Erlang]);

    let dir_fs = tempdir().unwrap();
    std::fs::write(dir_fs.path().join("App.fsproj"), "<Project></Project>").unwrap();
    assert_eq!(detect_engine(dir_fs.path()), EngineKind::Fsharp);
    assert_eq!(detect_all_engines(dir_fs.path()), vec![EngineKind::Fsharp]);

    let dir_pl = tempdir().unwrap();
    std::fs::write(dir_pl.path().join("cpanfile"), "requires 'Mojolicious';").unwrap();
    assert_eq!(detect_engine(dir_pl.path()), EngineKind::Perl);
    assert_eq!(detect_all_engines(dir_pl.path()), vec![EngineKind::Perl]);

    let dir_sol = tempdir().unwrap();
    std::fs::write(dir_sol.path().join("foundry.toml"), "[profile.default]").unwrap();
    assert_eq!(detect_engine(dir_sol.path()), EngineKind::Solidity);
    assert_eq!(
        detect_all_engines(dir_sol.path()),
        vec![EngineKind::Solidity]
    );

    let dir_nim = tempdir().unwrap();
    std::fs::write(dir_nim.path().join("pkg.nimble"), "version = \"0.1.0\"").unwrap();
    assert_eq!(detect_engine(dir_nim.path()), EngineKind::Nim);
    assert_eq!(detect_all_engines(dir_nim.path()), vec![EngineKind::Nim]);

    let dir_d = tempdir().unwrap();
    std::fs::write(dir_d.path().join("dub.json"), "{\"name\": \"pkg\"}").unwrap();
    assert_eq!(detect_engine(dir_d.path()), EngineKind::D);
    assert_eq!(detect_all_engines(dir_d.path()), vec![EngineKind::D]);

    let dir_f = tempdir().unwrap();
    std::fs::write(dir_f.path().join("fpm.toml"), "name = \"pkg\"").unwrap();
    assert_eq!(detect_engine(dir_f.path()), EngineKind::Fortran);
    assert_eq!(detect_all_engines(dir_f.path()), vec![EngineKind::Fortran]);

    let dir_sql = tempdir().unwrap();
    std::fs::write(dir_sql.path().join(".sqlfluff"), "[sqlfluff]").unwrap();
    assert_eq!(detect_engine(dir_sql.path()), EngineKind::Sql);
    assert_eq!(detect_all_engines(dir_sql.path()), vec![EngineKind::Sql]);

    let dir_gql = tempdir().unwrap();
    std::fs::write(dir_gql.path().join("codegen.yml"), "schema: schema.graphql").unwrap();
    assert_eq!(detect_engine(dir_gql.path()), EngineKind::Graphql);
    assert_eq!(
        detect_all_engines(dir_gql.path()),
        vec![EngineKind::Graphql]
    );

    let dir_proto = tempdir().unwrap();
    std::fs::write(dir_proto.path().join("buf.yaml"), "version: v1").unwrap();
    assert_eq!(detect_engine(dir_proto.path()), EngineKind::Protobuf);
    assert_eq!(
        detect_all_engines(dir_proto.path()),
        vec![EngineKind::Protobuf]
    );

    let dir_cr = tempdir().unwrap();
    std::fs::write(dir_cr.path().join("shard.yml"), "name: shard").unwrap();
    assert_eq!(detect_engine(dir_cr.path()), EngineKind::Crystal);
    assert_eq!(detect_all_engines(dir_cr.path()), vec![EngineKind::Crystal]);

    let dir_groovy = tempdir().unwrap();
    std::fs::write(dir_groovy.path().join("Jenkinsfile"), "pipeline {}").unwrap();
    assert_eq!(detect_engine(dir_groovy.path()), EngineKind::Groovy);
    assert_eq!(
        detect_all_engines(dir_groovy.path()),
        vec![EngineKind::Groovy]
    );

    let dir_ada = tempdir().unwrap();
    std::fs::write(
        dir_ada.path().join("default.gpr"),
        "project Default is end Default;",
    )
    .unwrap();
    assert_eq!(detect_engine(dir_ada.path()), EngineKind::Ada);
    assert_eq!(detect_all_engines(dir_ada.path()), vec![EngineKind::Ada]);

    let dir_v = tempdir().unwrap();
    std::fs::write(dir_v.path().join("v.mod"), "Module { name: 'pkg' }").unwrap();
    assert_eq!(detect_engine(dir_v.path()), EngineKind::V);
    assert_eq!(detect_all_engines(dir_v.path()), vec![EngineKind::V]);

    let dir_rkt = tempdir().unwrap();
    std::fs::write(dir_rkt.path().join("info.rkt"), "#lang info").unwrap();
    assert_eq!(detect_engine(dir_rkt.path()), EngineKind::Racket);
    assert_eq!(detect_all_engines(dir_rkt.path()), vec![EngineKind::Racket]);
}
