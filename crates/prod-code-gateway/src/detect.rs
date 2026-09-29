//! Multi-language workspace detection.
//!
//! Automatically identifies the language engine suited for a given workspace
//! based on project manifests (Cargo.toml, go.mod, package.json, pyproject.toml, etc.).

use prod_code_protocol::messages::EngineKind;
use std::path::Path;
use std::str::FromStr;

/// Manifest markers used to identify project language types.
const RUST_MARKERS: &[&str] = &["Cargo.toml"];
const GO_MARKERS: &[&str] = &["go.mod", "go.work"];
const PYTHON_MARKERS: &[&str] = &[
    "pyproject.toml",
    "requirements.txt",
    "setup.py",
    "setup.cfg",
    "Pipfile",
];
const CPP_MARKERS: &[&str] = &[
    "compile_commands.json",
    "CMakeLists.txt",
    "meson.build",
    ".clangd",
];
const SWIFT_MARKERS: &[&str] = &["Package.swift"];
const TYPESCRIPT_MARKERS: &[&str] = &[
    "tsconfig.json",
    "package.json",
    "jsconfig.json",
    "deno.json",
    "deno.jsonc",
];
const JAVA_MARKERS: &[&str] = &["pom.xml", "build.gradle"];
const KOTLIN_MARKERS: &[&str] = &["build.gradle.kts", "settings.gradle.kts"];
const CSHARP_MARKERS: &[&str] = &["global.json"];
const PHP_MARKERS: &[&str] = &["composer.json"];
const RUBY_MARKERS: &[&str] = &["Gemfile"];
const DART_MARKERS: &[&str] = &["pubspec.yaml"];
const ZIG_MARKERS: &[&str] = &["build.zig", "build.zig.zon"];
const ELIXIR_MARKERS: &[&str] = &["mix.exs"];
const SCALA_MARKERS: &[&str] = &["build.sbt"];
const LUA_MARKERS: &[&str] = &[".luarc.json", ".luacheckrc"];
const HASKELL_MARKERS: &[&str] = &["cabal.project", "stack.yaml", "package.yaml"];
const OCAML_MARKERS: &[&str] = &["dune-project", "dune"];
const CLOJURE_MARKERS: &[&str] = &["project.clj", "deps.edn"];
const JULIA_MARKERS: &[&str] = &["JuliaProject.toml"];
const SHELL_MARKERS: &[&str] = &[".shellcheckrc"];
const R_MARKERS: &[&str] = &["DESCRIPTION", "NAMESPACE"];
const ERLANG_MARKERS: &[&str] = &["rebar.config", "rebar.lock", "erlang.mk"];
const PERL_MARKERS: &[&str] = &["cpanfile", "Makefile.PL", "Build.PL", "dist.ini"];
const SOLIDITY_MARKERS: &[&str] = &[
    "foundry.toml",
    "hardhat.config.js",
    "hardhat.config.ts",
    "hardhat.config.cjs",
    "truffle-config.js",
];
const NIM_MARKERS: &[&str] = &["nim.cfg"];
const D_MARKERS: &[&str] = &["dub.json", "dub.sdl"];
const FORTRAN_MARKERS: &[&str] = &["fpm.toml"];
const SQL_MARKERS: &[&str] = &[".sqlfluff", "sqlfluff.cfg", ".sqls.json", "sqls.json", "schema.sql"];
const GRAPHQL_MARKERS: &[&str] = &[
    "codegen.yml",
    "codegen.ts",
    "codegen.json",
    ".graphqlrc",
    ".graphqlrc.yml",
    ".graphqlrc.json",
    "schema.graphql",
];
const PROTOBUF_MARKERS: &[&str] = &[
    "buf.yaml",
    "buf.work.yaml",
    "buf.gen.yaml",
    "buf.lock",
    ".protolint.yaml",
];
const CRYSTAL_MARKERS: &[&str] = &["shard.yml", "shard.lock"];
const GROOVY_MARKERS: &[&str] = &["Jenkinsfile"];
const ADA_MARKERS: &[&str] = &["default.gpr"];
const V_MARKERS: &[&str] = &["v.mod"];
const RACKET_MARKERS: &[&str] = &["info.rkt"];

/// The names a Makefile goes by.
const MAKEFILES: &[&str] = &["Makefile", "makefile", "GNUmakefile"];

/// The extensions of C and C++ sources and headers.
const C_SOURCE_EXTENSIONS: &[&str] = &["c", "cc", "cpp", "cxx", "h", "hh", "hpp", "hxx"];

/// A Makefile next to C or C++ sources, at the root or in `src/`: a C/C++ project built with
/// Make (#404). It ranks below every other manifest, since Go, Python and JavaScript
/// repositories keep a Makefile of tasks too, and a Makefile with no C sources is not C.
pub fn is_make_cpp_project(root: &Path) -> bool {
    let has_c_sources = |dir: &Path| {
        std::fs::read_dir(dir).is_ok_and(|entries| {
            entries.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .is_some_and(|x| C_SOURCE_EXTENSIONS.contains(&x))
            })
        })
    };
    MAKEFILES.iter().any(|m| root.join(m).is_file())
        && (has_c_sources(root) || has_c_sources(&root.join("src")))
}

/// An XcodeGen spec at the root: a `project.yml` with top-level `targets:`, from which the
/// Xcode project, usually not committed, is generated (#404).
pub fn is_xcodegen_spec(root: &Path) -> bool {
    std::fs::read_to_string(root.join("project.yml"))
        .is_ok_and(|text| text.lines().any(|line| line.starts_with("targets:")))
}

/// A Swift package manifest, an XcodeGen spec, or an Xcode project/workspace bundle at the root.
pub fn has_swift_project(root: &Path) -> bool {
    if SWIFT_MARKERS.iter().any(|m| root.join(m).exists()) || is_xcodegen_spec(root) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".xcodeproj") || name.ends_with(".xcworkspace")
            })
        })
        .unwrap_or(false)
}

/// A C# project (.csproj, .sln) or global.json configuration at the root.
pub fn has_csharp_project(root: &Path) -> bool {
    if CSHARP_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".csproj") || name.ends_with(".sln")
            })
        })
        .unwrap_or(false)
}

/// A Lua project (.luarc.json, .luacheckrc, *.rockspec) at the root.
pub fn has_lua_project(root: &Path) -> bool {
    if LUA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".rockspec")
            })
        })
        .unwrap_or(false)
}

/// A Haskell project (cabal.project, stack.yaml, package.yaml, *.cabal) at the root.
pub fn has_haskell_project(root: &Path) -> bool {
    if HASKELL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".cabal")
            })
        })
        .unwrap_or(false)
}

/// An OCaml project (dune-project, dune, *.opam) at the root.
pub fn has_ocaml_project(root: &Path) -> bool {
    if OCAML_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".opam")
            })
        })
        .unwrap_or(false)
}

/// A Clojure project (project.clj, deps.edn) at the root.
pub fn has_clojure_project(root: &Path) -> bool {
    CLOJURE_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Julia project (Project.toml, JuliaProject.toml) at the root.
pub fn has_julia_project(root: &Path) -> bool {
    if JULIA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    let proj = root.join("Project.toml");
    if proj.is_file()
        && let Ok(text) = std::fs::read_to_string(&proj)
        && (text.contains("[deps]") || text.contains("uuid ="))
    {
        return true;
    }
    false
}

/// A Shell project (.shellcheckrc, *.sh) at the root.
pub fn has_shell_project(root: &Path) -> bool {
    if SHELL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name();
                let name = name.to_string_lossy();
                name.ends_with(".sh")
            })
        })
        .unwrap_or(false)
}

/// An R project (DESCRIPTION, NAMESPACE) at the root.
pub fn has_r_project(root: &Path) -> bool {
    R_MARKERS.iter().any(|m| root.join(m).exists())
}

/// An Erlang project (rebar.config, rebar.lock, erlang.mk) at the root.
pub fn has_erlang_project(root: &Path) -> bool {
    ERLANG_MARKERS.iter().any(|m| root.join(m).exists())
}

/// An F# project (*.fsproj) at the root.
pub fn has_fsharp_project(root: &Path) -> bool {
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with(".fsproj")
            })
        })
        .unwrap_or(false)
}

/// A Perl project (cpanfile, Makefile.PL, Build.PL, dist.ini) at the root.
pub fn has_perl_project(root: &Path) -> bool {
    PERL_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Solidity project (foundry.toml, hardhat.config.*) at the root.
pub fn has_solidity_project(root: &Path) -> bool {
    SOLIDITY_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Nim project (*.nimble, nim.cfg) at the root.
pub fn has_nim_project(root: &Path) -> bool {
    if NIM_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with(".nimble")
            })
        })
        .unwrap_or(false)
}

/// A D project (dub.json, dub.sdl) at the root.
pub fn has_d_project(root: &Path) -> bool {
    D_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A Fortran project (fpm.toml) at the root.
pub fn has_fortran_project(root: &Path) -> bool {
    FORTRAN_MARKERS.iter().any(|m| root.join(m).exists())
}

/// A SQL project (.sqlfluff, sqlfluff.cfg, .sqls.json, schema.sql, or *.sql) at the root.
pub fn has_sql_project(root: &Path) -> bool {
    if SQL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with(".sql")
            })
        })
        .unwrap_or(false)
}

/// A GraphQL project (codegen.yml, .graphqlrc, schema.graphql, or *.graphql) at the root.
pub fn has_graphql_project(root: &Path) -> bool {
    if GRAPHQL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".graphql") || name.ends_with(".gql")
            })
        })
        .unwrap_or(false)
}

/// A Protobuf project (buf.yaml, buf.gen.yaml, .protolint.yaml, or *.proto) at the root.
pub fn has_protobuf_project(root: &Path) -> bool {
    if PROTOBUF_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with(".proto")
            })
        })
        .unwrap_or(false)
}

/// A Crystal project (shard.yml, shard.lock, or *.cr) at the root.
pub fn has_crystal_project(root: &Path) -> bool {
    if CRYSTAL_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with(".cr")
            })
        })
        .unwrap_or(false)
}

/// A Groovy project (Jenkinsfile, *.groovy, *.gvy) at the root.
pub fn has_groovy_project(root: &Path) -> bool {
    if GROOVY_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".groovy") || name.ends_with(".gvy")
            })
        })
        .unwrap_or(false)
}

/// An Ada project (*.gpr, *.adb, *.ads) at the root.
pub fn has_ada_project(root: &Path) -> bool {
    if ADA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".gpr") || name.ends_with(".adb") || name.ends_with(".ads")
            })
        })
        .unwrap_or(false)
}

/// A V project (v.mod or *.v) at the root.
pub fn has_v_project(root: &Path) -> bool {
    if V_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.ends_with(".v") || name.ends_with(".vsh")
            })
        })
        .unwrap_or(false)
}

/// A Racket project (info.rkt or *.rkt) at the root.
pub fn has_racket_project(root: &Path) -> bool {
    if RACKET_MARKERS.iter().any(|m| root.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(root)
        .map(|entries| {
            entries.flatten().any(|e| {
                e.file_name().to_string_lossy().ends_with(".rkt")
            })
        })
        .unwrap_or(false)
}

/// Detect the primary engine kind for the specified workspace path.
///
/// Priority order:
/// 1. Rust (`Cargo.toml`)
/// 2. Go (`go.mod`, `go.work`)
/// 3. Swift (`Package.swift`, an XcodeGen `project.yml`, an Xcode bundle)
/// 4. C/C++ (`compile_commands.json`, `CMakeLists.txt`, `meson.build`, `.clangd`)
/// 5. Python (`pyproject.toml`, `requirements.txt`, `setup.py`, etc.)
/// 6. TypeScript / JavaScript (`tsconfig.json`, `package.json`, etc.)
/// 7. C/C++ built with Make (a Makefile next to C sources)
/// 8. Kotlin (`build.gradle.kts`, `settings.gradle.kts`)
/// 9. Java (`pom.xml`, `build.gradle`)
/// 10. C# (`*.csproj`, `*.sln`, `global.json`)
/// 11. PHP (`composer.json`)
/// 12. Ruby (`Gemfile`)
/// 13. Generic LSP fallback
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
    for marker in TYPESCRIPT_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::TypeScript;
        }
    }
    if is_make_cpp_project(root) {
        return EngineKind::Cpp;
    }
    if KOTLIN_MARKERS.iter().any(|m| root.join(m).exists()) {
        return EngineKind::Kotlin;
    }
    for marker in JAVA_MARKERS {
        if root.join(marker).exists() {
            return EngineKind::Java;
        }
    }
    if has_csharp_project(root) {
        return EngineKind::Csharp;
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
    if SCALA_MARKERS.iter().any(|m| root.join(m).exists()) {
        return EngineKind::Scala;
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
    if has_shell_project(root) {
        return EngineKind::Shell;
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
    EngineKind::Generic
}

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
    if KOTLIN_MARKERS.iter().any(|m| root.join(m).exists()) {
        engines.push(EngineKind::Kotlin);
    }
    if JAVA_MARKERS.iter().any(|m| root.join(m).exists()) {
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
    if has_groovy_project(root) {
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

    if engines.is_empty() {
        engines.push(EngineKind::Generic);
    }

    engines
}

/// Resolve the effective engine, honoring an explicit client preference if provided.
pub fn resolve_engine(root: &Path, preferred: Option<&str>) -> EngineKind {
    if let Some(pref) = preferred.filter(|p| !p.trim().is_empty()) {
        return EngineKind::from_str(pref.trim()).unwrap_or(EngineKind::Generic);
    }
    detect_engine(root)
}

#[cfg(test)]
mod tests {
    use super::*;
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
            vec![EngineKind::Rust, EngineKind::Go, EngineKind::TypeScript]
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
        std::fs::write(dir_kt.path().join("build.gradle.kts"), "").unwrap();
        assert_eq!(detect_engine(dir_kt.path()), EngineKind::Kotlin);
        assert_eq!(detect_all_engines(dir_kt.path()), vec![EngineKind::Kotlin]);

        let dir_cs = tempdir().unwrap();
        std::fs::write(dir_cs.path().join("App.csproj"), "<Project></Project>").unwrap();
        assert_eq!(detect_engine(dir_cs.path()), EngineKind::Csharp);
        assert_eq!(detect_all_engines(dir_cs.path()), vec![EngineKind::Csharp]);

        let dir_php = tempdir().unwrap();
        std::fs::write(dir_php.path().join("composer.json"), "{}").unwrap();
        assert_eq!(detect_engine(dir_php.path()), EngineKind::Php);
        assert_eq!(detect_all_engines(dir_php.path()), vec![EngineKind::Php]);

        let dir_rb = tempdir().unwrap();
        std::fs::write(dir_rb.path().join("Gemfile"), "source 'https://rubygems.org'").unwrap();
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
        assert_eq!(detect_all_engines(dir_scala.path()), vec![EngineKind::Scala]);

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
        assert_eq!(detect_all_engines(dir_clj.path()), vec![EngineKind::Clojure]);

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
        assert_eq!(detect_all_engines(dir_sol.path()), vec![EngineKind::Solidity]);

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
        assert_eq!(detect_all_engines(dir_gql.path()), vec![EngineKind::Graphql]);

        let dir_proto = tempdir().unwrap();
        std::fs::write(dir_proto.path().join("buf.yaml"), "version: v1").unwrap();
        assert_eq!(detect_engine(dir_proto.path()), EngineKind::Protobuf);
        assert_eq!(detect_all_engines(dir_proto.path()), vec![EngineKind::Protobuf]);

        let dir_cr = tempdir().unwrap();
        std::fs::write(dir_cr.path().join("shard.yml"), "name: shard").unwrap();
        assert_eq!(detect_engine(dir_cr.path()), EngineKind::Crystal);
        assert_eq!(detect_all_engines(dir_cr.path()), vec![EngineKind::Crystal]);

        let dir_groovy = tempdir().unwrap();
        std::fs::write(dir_groovy.path().join("Jenkinsfile"), "pipeline {}").unwrap();
        assert_eq!(detect_engine(dir_groovy.path()), EngineKind::Groovy);
        assert_eq!(detect_all_engines(dir_groovy.path()), vec![EngineKind::Groovy]);

        let dir_ada = tempdir().unwrap();
        std::fs::write(dir_ada.path().join("default.gpr"), "project Default is end Default;").unwrap();
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

    #[test]
    fn test_universal_language_detection_matrix_all_40_languages() {
        struct Case {
            lang: &'static str,
            files: &'static [(&'static str, &'static str)],
            kind: EngineKind,
        }

        let matrix = [
            Case { lang: "Rust", files: &[("Cargo.toml", "[workspace]")], kind: EngineKind::Rust },
            Case { lang: "Go", files: &[("go.mod", "module test")], kind: EngineKind::Go },
            Case { lang: "Python", files: &[("pyproject.toml", "[project]")], kind: EngineKind::Python },
            Case { lang: "TypeScript", files: &[("tsconfig.json", "{}")], kind: EngineKind::TypeScript },
            Case { lang: "Cpp", files: &[("CMakeLists.txt", "project(test)")], kind: EngineKind::Cpp },
            Case { lang: "Swift", files: &[("Package.swift", "// swift-tools-version:5.9")], kind: EngineKind::Swift },
            Case { lang: "Java", files: &[("pom.xml", "<project></project>")], kind: EngineKind::Java },
            Case { lang: "Kotlin", files: &[("build.gradle.kts", "")], kind: EngineKind::Kotlin },
            Case { lang: "Csharp", files: &[("global.json", "{}")], kind: EngineKind::Csharp },
            Case { lang: "Php", files: &[("composer.json", "{}")], kind: EngineKind::Php },
            Case { lang: "Ruby", files: &[("Gemfile", "")], kind: EngineKind::Ruby },
            Case { lang: "Dart", files: &[("pubspec.yaml", "name: test")], kind: EngineKind::Dart },
            Case { lang: "Zig", files: &[("build.zig", "")], kind: EngineKind::Zig },
            Case { lang: "Elixir", files: &[("mix.exs", "defmodule M do end")], kind: EngineKind::Elixir },
            Case { lang: "Scala", files: &[("build.sbt", "")], kind: EngineKind::Scala },
            Case { lang: "Lua", files: &[(".luarc.json", "{}")], kind: EngineKind::Lua },
            Case { lang: "Haskell", files: &[("cabal.project", "")], kind: EngineKind::Haskell },
            Case { lang: "Ocaml", files: &[("dune-project", "(lang dune 3.0)")], kind: EngineKind::Ocaml },
            Case { lang: "Clojure", files: &[("project.clj", "")], kind: EngineKind::Clojure },
            Case { lang: "Julia", files: &[("JuliaProject.toml", "")], kind: EngineKind::Julia },
            Case { lang: "Shell", files: &[(".shellcheckrc", "")], kind: EngineKind::Shell },
            Case { lang: "R", files: &[("DESCRIPTION", "Package: test")], kind: EngineKind::R },
            Case { lang: "Erlang", files: &[("rebar.config", "")], kind: EngineKind::Erlang },
            Case { lang: "Fsharp", files: &[("App.fsproj", "")], kind: EngineKind::Fsharp },
            Case { lang: "Perl", files: &[("cpanfile", "")], kind: EngineKind::Perl },
            Case { lang: "Solidity", files: &[("foundry.toml", "")], kind: EngineKind::Solidity },
            Case { lang: "Nim", files: &[("nim.cfg", "")], kind: EngineKind::Nim },
            Case { lang: "D", files: &[("dub.json", "{}")], kind: EngineKind::D },
            Case { lang: "Fortran", files: &[("fpm.toml", "")], kind: EngineKind::Fortran },
            Case { lang: "Sql", files: &[(".sqlfluff", "")], kind: EngineKind::Sql },
            Case { lang: "Graphql", files: &[("codegen.yml", "")], kind: EngineKind::Graphql },
            Case { lang: "Protobuf", files: &[("buf.yaml", "")], kind: EngineKind::Protobuf },
            Case { lang: "Crystal", files: &[("shard.yml", "")], kind: EngineKind::Crystal },
            Case { lang: "Groovy", files: &[("Jenkinsfile", "")], kind: EngineKind::Groovy },
            Case { lang: "Ada", files: &[("default.gpr", "")], kind: EngineKind::Ada },
            Case { lang: "V", files: &[("v.mod", "")], kind: EngineKind::V },
            Case { lang: "Racket", files: &[("info.rkt", "")], kind: EngineKind::Racket },
            Case { lang: "Generic", files: &[], kind: EngineKind::Generic },
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
}
