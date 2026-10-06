/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Result, anyhow};

use super::super::types::{CppBuild, ProjectTools, PythonRuntime, VerifyKind};

pub fn strs(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

/// The shell script that runs clang-tidy over a C/C++ project's sources with the build's
/// compilation database (configured first), with `-fix` when `fix`. A Make project has no
/// database to give it, and is refused.
pub fn clang_tidy_script(build: CppBuild, fix: bool) -> Result<String> {
    let configure = match build {
        CppBuild::CMake => "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON >/dev/null",
        CppBuild::Meson => "{ [ -d build ] || meson setup build >/dev/null; }",
        CppBuild::Make => {
            return Err(anyhow!(
                "clang-tidy needs a compilation database, which a Make build does not write; \
                 use CMake or Meson, or generate one with bear"
            ));
        }
    };
    Ok(format!(
        "{configure} && find . -path ./build -prune -o \\( -name '*.c' -o -name '*.cc' -o -name '*.cpp' -o -name '*.cxx' \\) -print | xargs -r clang-tidy -p build --quiet{}",
        if fix { " -fix" } else { "" }
    ))
}

/// The command that applies a linter's own fixes in place, for `lint --fix` on a language
/// whose linter has a fix mode: `ruff check --fix`, `eslint --fix`, `biome lint --write`,
/// `clang-tidy -fix`, and `golangci-lint run --fix` for a Go project with a golangci config.
/// `None` when it has none (`go vet`), or for Rust, whose fixes are read from the compiler's
/// JSON instead.
pub fn fix_command(tools: &ProjectTools, language: &str) -> Result<Option<Vec<String>>> {
    let pm = tools.package_manager;
    Ok(Some(match language {
        "python" => {
            let mut c = if tools.python == PythonRuntime::Uv {
                strs(&["uv", "run", "ruff"])
            } else {
                strs(&["ruff"])
            };
            c.extend(strs(&["check", ".", "--fix", "--output-format", "concise"]));
            c
        }
        "typescript" => match tools.js_linter {
            Some("eslint") => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["eslint", ".", "--fix", "-f", "unix"]));
                c
            }
            Some("biome") => {
                let mut c = strs(&pm.exec());
                c.extend(strs(&["biome", "lint", "--write", "."]));
                c
            }
            _ => return Ok(None),
        },
        "cpp" => strs(&["sh", "-c", &clang_tidy_script(tools.cpp, true)?]),
        "go" if tools.golangci_config => strs(&["golangci-lint", "run", "--fix", "./..."]),
        _ => return Ok(None),
    }))
}

/// Commands that do not depend on detected tooling (Rust, Go, Swift packages).
pub fn plan_command_basic(
    language: &str,
    kind: VerifyKind,
    filter: Option<&str>,
) -> Result<Vec<String>> {
    let mut cmd: Vec<String> = match (language, kind) {
        ("rust", VerifyKind::Check) => vec![
            "cargo",
            "check",
            "--workspace",
            "--all-targets",
            "--message-format=json",
        ],
        ("rust", VerifyKind::Lint) => vec![
            "cargo",
            "clippy",
            "--workspace",
            "--all-targets",
            "--message-format=json",
            "--",
            "-D",
            "warnings",
        ],
        ("rust", VerifyKind::Test) => vec!["cargo", "test", "--workspace"],
        ("rust", VerifyKind::Bench) => vec!["cargo", "bench", "--workspace"],
        ("go", VerifyKind::Bench) => vec!["go", "test", "-run", "^$", "-bench"],
        ("go", VerifyKind::Check) => vec!["go", "build", "./..."],
        ("go", VerifyKind::Test) => vec!["go", "test", "-json", "./..."],
        ("swift", VerifyKind::Check) => vec!["swift", "build", "--build-tests"],
        ("swift", VerifyKind::Test) => vec!["swift", "test"],
        ("csharp", VerifyKind::Check) => vec!["dotnet", "build"],
        ("csharp", VerifyKind::Test) => vec!["dotnet", "test"],
        ("java", VerifyKind::Check) => vec!["mvn", "test-compile"],
        ("java", VerifyKind::Test) => vec!["mvn", "test"],
        ("kotlin", VerifyKind::Check) => vec!["gradle", "compileKotlin"],
        ("kotlin", VerifyKind::Test) => vec!["gradle", "test"],
        ("php", VerifyKind::Check) => vec![
            "sh",
            "-c",
            r#"if ! find . -type f \( -name '*.php' -o -name '*.phtml' \) -print -quit | grep -q .; then
    echo 'no PHP source files found' >&2
    exit 1
fi
find . -type f \( -name '*.php' -o -name '*.phtml' \) -print0 | xargs -0 -n1 php -l"#,
        ],
        ("php", VerifyKind::Test) => vec!["phpunit"],
        ("ruby", VerifyKind::Check) => vec!["bundle", "exec", "rake", "test"],
        ("ruby", VerifyKind::Test) => vec!["bundle", "exec", "rake", "test"],
        ("dart", VerifyKind::Check) => vec!["dart", "analyze"],
        ("dart", VerifyKind::Test) => vec!["dart", "test"],
        ("zig", VerifyKind::Check) => vec!["zig", "build"],
        ("zig", VerifyKind::Test) => vec!["zig", "test"],
        ("elixir", VerifyKind::Check) => vec!["mix", "compile"],
        ("elixir", VerifyKind::Test) => vec!["mix", "test"],
        ("scala", VerifyKind::Check) => vec!["sbt", "compile"],
        ("scala", VerifyKind::Test) => vec!["sbt", "test"],
        ("lua", VerifyKind::Check) => vec!["luacheck", "."],
        ("lua", VerifyKind::Test) => vec!["busted"],
        ("haskell", VerifyKind::Check) => vec!["cabal", "build"],
        ("haskell", VerifyKind::Test) => vec!["cabal", "test"],
        ("ocaml", VerifyKind::Check) => vec!["dune", "build"],
        ("ocaml", VerifyKind::Test) => vec!["dune", "runtest"],
        ("clojure", VerifyKind::Check) => vec!["lein", "check"],
        ("clojure", VerifyKind::Test) => vec!["lein", "test"],
        ("julia", VerifyKind::Check) => vec!["julia", "--project", "-e", "using Pkg; Pkg.build()"],
        ("julia", VerifyKind::Test) => vec!["julia", "--project", "-e", "using Pkg; Pkg.test()"],
        ("shell", VerifyKind::Check) => vec!["shellcheck", "**/*.sh"],
        ("shell", VerifyKind::Test) => vec!["bats", "test"],
        ("r", VerifyKind::Check) => vec!["R", "CMD", "check", "."],
        ("r", VerifyKind::Test) => vec!["R", "-e", "testthat::test_dir('tests')"],
        ("erlang", VerifyKind::Check) => vec!["rebar3", "compile"],
        ("erlang", VerifyKind::Test) => vec!["rebar3", "eunit"],
        ("fsharp", VerifyKind::Check) => vec!["dotnet", "build"],
        ("fsharp", VerifyKind::Test) => vec!["dotnet", "test"],
        ("perl", VerifyKind::Check) => vec![
            "sh",
            "-c",
            r#"if ! find . -type f \( -name '*.pl' -o -name '*.pm' -o -name '*.t' \) -print -quit | grep -q .; then
    echo 'no Perl source files found' >&2
    exit 1
fi
find . -type f \( -name '*.pl' -o -name '*.pm' -o -name '*.t' \) -print0 | xargs -0 -n1 perl -Ilib -c"#,
        ],
        ("perl", VerifyKind::Test) => vec!["prove", "-l"],
        ("solidity", VerifyKind::Check) => vec!["forge", "build"],
        ("solidity", VerifyKind::Test) => vec!["forge", "test"],
        ("nim", VerifyKind::Check) => vec!["nim", "check"],
        ("nim", VerifyKind::Test) => vec!["nimble", "test"],
        ("d", VerifyKind::Check) => vec!["dub", "build"],
        ("d", VerifyKind::Test) => vec!["dub", "test"],
        ("fortran", VerifyKind::Check) => vec!["fpm", "build"],
        ("fortran", VerifyKind::Test) => vec!["fpm", "test"],
        ("sql", VerifyKind::Check) => vec!["sqlfluff", "lint"],
        ("sql", VerifyKind::Test) => vec!["pg_prove"],
        ("graphql", VerifyKind::Check) => vec!["graphql-codegen", "--check"],
        ("graphql", VerifyKind::Test) => vec!["graphql-codegen"],
        ("protobuf", VerifyKind::Check) => vec!["buf", "lint"],
        ("protobuf", VerifyKind::Test) => vec!["buf", "breaking", "--against", ".git#branch=main"],
        ("crystal", VerifyKind::Check) => vec!["crystal", "build", "--no-codegen"],
        ("crystal", VerifyKind::Test) => vec!["crystal", "spec"],
        ("groovy", VerifyKind::Check) => vec!["gradle", "compileGroovy"],
        ("groovy", VerifyKind::Test) => vec!["gradle", "test"],
        ("ada", VerifyKind::Check) => vec!["gprbuild", "-c"],
        ("ada", VerifyKind::Test) => vec!["gprtest"],
        ("v", VerifyKind::Check) => vec!["v", "check", "."],
        ("v", VerifyKind::Test) => vec!["v", "test", "."],
        ("racket", VerifyKind::Check) => vec!["raco", "make"],
        ("racket", VerifyKind::Test) => vec!["raco", "test", "."],
        ("terraform", VerifyKind::Check) => vec!["terraform", "validate"],
        ("terraform", VerifyKind::Test) => vec!["terraform", "test"],
        ("nix", VerifyKind::Check) => vec!["nix", "flake", "check"],
        ("nix", VerifyKind::Test) => vec!["nix", "flake", "check"],
        ("markdown", VerifyKind::Check) => vec!["markdownlint", "."],
        ("markdown", VerifyKind::Test) => vec!["markdownlint", "."],
        ("yaml", VerifyKind::Check) => vec!["yamllint", "."],
        ("yaml", VerifyKind::Test) => vec!["yamllint", "."],
        ("toml", VerifyKind::Check) => vec!["taplo", "check"],
        ("toml", VerifyKind::Test) => vec!["taplo", "check"],
        ("json", VerifyKind::Check) => vec!["jsonlint", "."],
        ("json", VerifyKind::Test) => vec!["jsonlint", "."],
        ("html", VerifyKind::Check) => vec!["htmlhint", "."],
        ("html", VerifyKind::Test) => vec!["htmlhint", "."],
        ("css", VerifyKind::Check) => vec!["stylelint", "**/*.css"],
        ("css", VerifyKind::Test) => vec!["stylelint", "**/*.css"],
        ("dockerfile", VerifyKind::Check) => vec!["hadolint", "Dockerfile"],
        ("dockerfile", VerifyKind::Test) => vec!["hadolint", "Dockerfile"],
        ("svelte", VerifyKind::Check) => vec!["svelte-check"],
        ("svelte", VerifyKind::Test) => vec!["svelte-check"],
        ("vue", VerifyKind::Check) => vec!["vue-tsc", "--noEmit"],
        ("vue", VerifyKind::Test) => vec!["vue-tsc", "--noEmit"],
        ("assembly", VerifyKind::Check) | ("assembly", VerifyKind::Test) => vec![
            "sh",
            "-c",
            r#"if ! find . -type f \( -name '*.asm' -o -name '*.nasm' -o -name '*.s' -o -name '*.S' \) -print -quit | grep -q .; then
    echo 'no Assembly source files found' >&2
    exit 1
fi
find . -type f \( -name '*.asm' -o -name '*.nasm' -o -name '*.s' -o -name '*.S' \) -print0 | xargs -0 -n1 sh -c 'case "$0" in *.asm|*.nasm) nasm -f elf64 -o /dev/null "$0" ;; *.s|*.S) as --64 -o /dev/null "$0" ;; esac'"#,
        ],
        _ => {
            return Err(anyhow!(
                "no {} command for language {language}",
                kind.label()
            ));
        }
    }
    .into_iter()
    .map(str::to_string)
    .collect();
    if let Some(filter) = filter.filter(|f| !f.is_empty()) {
        match (language, kind) {
            ("rust", VerifyKind::Test) => cmd.push(filter.to_string()),
            ("go", VerifyKind::Test) => {
                cmd.push("-run".to_string());
                cmd.push(filter.to_string());
            }
            ("swift", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            ("csharp", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            ("java", VerifyKind::Test) => {
                cmd.push(format!("-Dtest={}", filter));
            }
            ("kotlin", VerifyKind::Test) => {
                cmd.push("--tests".to_string());
                cmd.push(filter.to_string());
            }
            ("php", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            ("dart", VerifyKind::Test) => {
                cmd.push("--name".to_string());
                cmd.push(filter.to_string());
            }
            ("zig", VerifyKind::Test) => {
                cmd.push("--test-filter".to_string());
                cmd.push(filter.to_string());
            }
            ("elixir", VerifyKind::Test) => {
                cmd.push("--only".to_string());
                cmd.push(filter.to_string());
            }
            ("scala", VerifyKind::Test) => {
                cmd.push(format!("testOnly *{}", filter));
            }
            ("lua", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            ("haskell", VerifyKind::Test) => {
                cmd.push("--test-show-details=direct".to_string());
                cmd.push(format!("--test-option=-m{}", filter));
            }
            ("clojure", VerifyKind::Test) => {
                cmd.push(format!(":only {}", filter));
            }
            ("shell", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("r", VerifyKind::Test) => {
                cmd.push(format!("-filter={}", filter));
            }
            ("erlang", VerifyKind::Test) => {
                cmd.push(format!("--module={}", filter));
            }
            ("fsharp", VerifyKind::Test) => {
                cmd.push("--filter".to_string());
                cmd.push(filter.to_string());
            }
            ("perl", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("solidity", VerifyKind::Test) => {
                cmd.push("--match-test".to_string());
                cmd.push(filter.to_string());
            }
            ("d", VerifyKind::Test) => {
                cmd.push("--".to_string());
                cmd.push(filter.to_string());
            }
            ("fortran", VerifyKind::Test) => {
                cmd.push("--target".to_string());
                cmd.push(filter.to_string());
            }
            ("sql", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("crystal", VerifyKind::Test) => {
                cmd.push("-e".to_string());
                cmd.push(filter.to_string());
            }
            ("groovy", VerifyKind::Test) => {
                cmd.push("--tests".to_string());
                cmd.push(filter.to_string());
            }
            ("ada", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("v", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("racket", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("terraform", VerifyKind::Test) => {
                cmd.push("-filter".to_string());
                cmd.push(filter.to_string());
            }
            ("markdown", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("yaml", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("toml", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("json", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("html", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("css", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("dockerfile", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("svelte", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("vue", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("assembly", VerifyKind::Test) => {
                cmd.push(filter.to_string());
            }
            ("rust", VerifyKind::Bench) => cmd.push(filter.to_string()),
            _ => {}
        }
    }
    // `go test -bench` takes the pattern right after it, then the packages.
    if (language, kind) == ("go", VerifyKind::Bench) {
        cmd.push(filter.filter(|f| !f.is_empty()).unwrap_or(".").to_string());
        cmd.push("./...".to_string());
    }
    Ok(cmd)
}
