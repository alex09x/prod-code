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
use RemoteExecCommand as C;
use RemoteExecFormat::{Json, Raw};
use RemoteExecLanguage as L;

fn req(language: L, command: C, format: RemoteExecFormat, args: &[&str]) -> RemoteExecRequest {
    RemoteExecRequest {
        client_workspace_root: "/w".into(),
        base_workspace_name: None,
        language,
        command,
        args: args.iter().map(|a| a.to_string()).collect(),
        env: Vec::new(),
        format,
        timeout_secs: 0,
        pull_changes: false,
        subdir: None,
        client_agent: None,
        client_host: None,
    }
}

fn custom(s: &str) -> C {
    C::Custom(s.into())
}

#[test]
fn argv_table_covers_every_language_command_and_format() {
    #[rustfmt::skip]
    #[allow(clippy::type_complexity)]
    let cases: Vec<(L, C, RemoteExecFormat, Vec<&str>, Vec<&str>)> = vec![
        (L::Rust, C::Check, Raw, vec![], vec!["cargo", "check", "--workspace", "--all-targets"]),
        (L::Rust, C::Check, Json, vec![], vec!["cargo", "check", "--workspace", "--all-targets", "--message-format=json"]),
        (L::Rust, C::Test, Raw, vec![], vec!["cargo", "test", "--workspace"]),
        (L::Rust, C::Test, Json, vec![], vec!["cargo", "test", "--workspace", "--message-format=json"]),
        (L::Rust, C::Lint, Raw, vec![], vec!["cargo", "clippy", "--workspace", "--all-targets"]),
        (L::Rust, C::Lint, Json, vec![], vec!["cargo", "clippy", "--workspace", "--all-targets", "--message-format=json"]),
        (L::Rust, C::Bench, Raw, vec![], vec!["cargo", "bench", "--workspace"]),
        (L::Rust, C::Bench, Json, vec![], vec!["cargo", "bench", "--workspace", "--message-format=json"]),
        (L::Rust, custom("cargo  fmt   --check"), Json, vec![], vec!["cargo", "fmt", "--check"]),
        (L::Go, C::Check, Raw, vec![], vec!["go", "vet", "./..."]),
        (L::Go, C::Check, Json, vec![], vec!["go", "vet", "./..."]),
        (L::Go, C::Test, Raw, vec![], vec!["go", "test", "-v", "./..."]),
        (L::Go, C::Test, Json, vec![], vec!["go", "test", "-json", "./..."]),
        (L::Go, C::Lint, Raw, vec![], vec!["golangci-lint", "run"]),
        (L::Go, C::Lint, Json, vec![], vec!["golangci-lint", "run", "--out-format=json"]),
        (L::Go, C::Bench, Raw, vec![], vec!["go", "test", "-run=^$", "-bench=.", "./..."]),
        (L::Go, custom("go generate ./..."), Raw, vec![], vec!["go", "generate", "./..."]),
        (L::TypeScript, C::Check, Raw, vec![], vec!["npx", "--no-install", "tsc", "--noEmit"]),
        (L::TypeScript, C::Test, Raw, vec![], vec!["npx", "--no-install", "vitest", "run"]),
        (L::TypeScript, C::Test, Json, vec![], vec!["npx", "--no-install", "vitest", "run", "--reporter=json"]),
        (L::TypeScript, C::Lint, Raw, vec![], vec!["npx", "--no-install", "eslint", "."]),
        (L::TypeScript, C::Lint, Json, vec![], vec!["npx", "--no-install", "eslint", ".", "--format=json"]),
        (L::TypeScript, C::Bench, Json, vec![], vec!["npx", "--no-install", "vitest", "bench", "run"]),
        (L::TypeScript, custom("npm run build"), Raw, vec![], vec!["npm", "run", "build"]),
        (L::Python, C::Check, Raw, vec![], vec!["python3", "-m", "compileall", "-q", "."]),
        (L::Python, C::Check, Raw, vec!["src"], vec!["python3", "-m", "compileall", "-q", "src"]),
        (L::Python, C::Test, Raw, vec![], vec!["pytest"]),
        (L::Python, C::Test, Json, vec![], vec!["pytest", "--json-report"]),
        (L::Python, C::Lint, Raw, vec![], vec!["ruff", "check", "."]),
        (L::Python, C::Lint, Json, vec![], vec!["ruff", "check", ".", "--output-format=json"]),
        (L::Python, C::Bench, Raw, vec![], vec!["pytest", "--benchmark-only"]),
        (L::Python, custom("mypy ."), Raw, vec![], vec!["mypy", "."]),
        (L::Cpp, C::Check, Raw, vec![], vec!["ninja", "-k", "0"]),
        (L::Cpp, C::Test, Json, vec![], vec!["ctest", "--output-on-failure"]),
        (L::Cpp, C::Lint, Raw, vec![], vec!["clang-tidy", "-p", "build"]),
        (L::Cpp, C::Bench, Raw, vec![], vec!["ninja", "bench"]),
        (L::Cpp, custom("make all"), Raw, vec![], vec!["make", "all"]),
        (L::Swift, C::Check, Raw, vec![], vec!["swift", "build"]),
        (L::Swift, C::Test, Raw, vec![], vec!["swift", "test"]),
        (L::Swift, C::Lint, Raw, vec![], vec!["swiftlint"]),
        (L::Swift, C::Lint, Json, vec![], vec!["swiftlint", "--reporter", "json"]),
        (L::Swift, C::Bench, Raw, vec![], vec!["swift", "run", "-c", "release", "bench"]),
        (L::Swift, custom("swift package clean"), Raw, vec![], vec!["swift", "package", "clean"]),
        (L::Generic, custom("make -j4  all"), Raw, vec![], vec!["make", "-j4", "all"]),
        (L::Generic, C::Check, Raw, vec![], vec!["check"]),
        (L::Generic, C::Test, Json, vec![], vec!["test"]),
        (L::Generic, C::Lint, Raw, vec![], vec!["lint"]),
        (L::Generic, C::Bench, Raw, vec![], vec!["bench"]),
        (L::Rust, C::Test, Raw, vec!["-p", "foo", "--", "--nocapture"], vec!["cargo", "test", "--workspace", "-p", "foo", "--", "--nocapture"]),
        (L::Generic, custom("   "), Raw, vec!["x"], vec!["x"]),
    ];
    for (language, command, format, args, want) in cases {
        let r = req(language, command.clone(), format, &args);
        assert_eq!(
            build_argv(&r),
            want,
            "{language} {command} {} {args:?}",
            format.as_str()
        );
        assert_eq!(r.to_argv(), want);
    }
}
