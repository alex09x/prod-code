/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{RemoteExecCommand, RemoteExecFormat, RemoteExecLanguage, RemoteExecRequest};

/// Constructs the toolchain command line (argv) for an execution request.
pub fn build_argv(req: &RemoteExecRequest) -> Vec<String> {
    let mut cmd = match (req.language, &req.command) {
        (RemoteExecLanguage::Rust, RemoteExecCommand::Check) => {
            let mut v = vec![
                "cargo".into(),
                "check".into(),
                "--workspace".into(),
                "--all-targets".into(),
            ];
            if req.format == RemoteExecFormat::Json {
                v.push("--message-format=json".into());
            }
            v
        }
        (RemoteExecLanguage::Rust, RemoteExecCommand::Test) => {
            let mut v = vec!["cargo".into(), "test".into(), "--workspace".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("--message-format=json".into());
            }
            v
        }
        (RemoteExecLanguage::Rust, RemoteExecCommand::Lint) => {
            let mut v = vec![
                "cargo".into(),
                "clippy".into(),
                "--workspace".into(),
                "--all-targets".into(),
            ];
            if req.format == RemoteExecFormat::Json {
                v.push("--message-format=json".into());
            }
            v
        }
        (RemoteExecLanguage::Rust, RemoteExecCommand::Bench) => {
            let mut v = vec!["cargo".into(), "bench".into(), "--workspace".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("--message-format=json".into());
            }
            v
        }
        (RemoteExecLanguage::Rust, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }

        (RemoteExecLanguage::Go, RemoteExecCommand::Check) => {
            vec!["go".into(), "vet".into(), "./...".into()]
        }
        (RemoteExecLanguage::Go, RemoteExecCommand::Test) => {
            let mut v = vec!["go".into(), "test".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("-json".into());
            } else {
                v.push("-v".into());
            }
            v.push("./...".into());
            v
        }
        (RemoteExecLanguage::Go, RemoteExecCommand::Lint) => {
            let mut v = vec!["golangci-lint".into(), "run".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("--out-format=json".into());
            }
            v
        }
        (RemoteExecLanguage::Go, RemoteExecCommand::Bench) => {
            vec![
                "go".into(),
                "test".into(),
                "-run=^$".into(),
                "-bench=.".into(),
                "./...".into(),
            ]
        }
        (RemoteExecLanguage::Go, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }

        (RemoteExecLanguage::TypeScript, RemoteExecCommand::Check) => {
            vec![
                "npx".into(),
                "--no-install".into(),
                "tsc".into(),
                "--noEmit".into(),
            ]
        }
        (RemoteExecLanguage::TypeScript, RemoteExecCommand::Test) => {
            let mut v = vec![
                "npx".into(),
                "--no-install".into(),
                "vitest".into(),
                "run".into(),
            ];
            if req.format == RemoteExecFormat::Json {
                v.push("--reporter=json".into());
            }
            v
        }
        (RemoteExecLanguage::TypeScript, RemoteExecCommand::Lint) => {
            let mut v = vec![
                "npx".into(),
                "--no-install".into(),
                "eslint".into(),
                ".".into(),
            ];
            if req.format == RemoteExecFormat::Json {
                v.push("--format=json".into());
            }
            v
        }
        (RemoteExecLanguage::TypeScript, RemoteExecCommand::Bench) => {
            vec![
                "npx".into(),
                "--no-install".into(),
                "vitest".into(),
                "bench".into(),
                "run".into(),
            ]
        }
        (RemoteExecLanguage::TypeScript, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }

        (RemoteExecLanguage::Python, RemoteExecCommand::Check) => {
            let mut v = vec![
                "python3".into(),
                "-m".into(),
                "compileall".into(),
                "-q".into(),
            ];
            if req.args.is_empty() {
                v.push(".".into());
            }
            v
        }
        (RemoteExecLanguage::Python, RemoteExecCommand::Test) => {
            let mut v = vec!["pytest".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("--json-report".into());
            }
            v
        }
        (RemoteExecLanguage::Python, RemoteExecCommand::Lint) => {
            let mut v = vec!["ruff".into(), "check".into(), ".".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("--output-format=json".into());
            }
            v
        }
        (RemoteExecLanguage::Python, RemoteExecCommand::Bench) => {
            vec!["pytest".into(), "--benchmark-only".into()]
        }
        (RemoteExecLanguage::Python, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }

        (RemoteExecLanguage::Cpp, RemoteExecCommand::Check) => {
            vec!["ninja".into(), "-k".into(), "0".into()]
        }
        (RemoteExecLanguage::Cpp, RemoteExecCommand::Test) => {
            vec!["ctest".into(), "--output-on-failure".into()]
        }
        (RemoteExecLanguage::Cpp, RemoteExecCommand::Lint) => {
            vec!["clang-tidy".into(), "-p".into(), "build".into()]
        }
        (RemoteExecLanguage::Cpp, RemoteExecCommand::Bench) => {
            vec!["ninja".into(), "bench".into()]
        }
        (RemoteExecLanguage::Cpp, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }

        (RemoteExecLanguage::Swift, RemoteExecCommand::Check) => {
            vec!["swift".into(), "build".into()]
        }
        (RemoteExecLanguage::Swift, RemoteExecCommand::Test) => {
            vec!["swift".into(), "test".into()]
        }
        (RemoteExecLanguage::Swift, RemoteExecCommand::Lint) => {
            let mut v = vec!["swiftlint".into()];
            if req.format == RemoteExecFormat::Json {
                v.push("--reporter".into());
                v.push("json".into());
            }
            v
        }
        (RemoteExecLanguage::Swift, RemoteExecCommand::Bench) => {
            vec![
                "swift".into(),
                "run".into(),
                "-c".into(),
                "release".into(),
                "bench".into(),
            ]
        }
        (RemoteExecLanguage::Swift, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }

        (RemoteExecLanguage::Generic, RemoteExecCommand::Custom(c)) => {
            c.split_whitespace().map(String::from).collect()
        }
        (RemoteExecLanguage::Generic, cmd) => {
            vec![cmd.as_str().to_string()]
        }
    };

    cmd.extend(req.args.clone());
    cmd
}
