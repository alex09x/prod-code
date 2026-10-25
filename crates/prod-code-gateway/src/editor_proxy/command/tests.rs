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

fn command(program: &str) -> ServerCommand {
    ServerCommand {
        program: program.to_string(),
        args: vec!["--stdio".to_string()],
        env: vec![("KEY".to_string(), "value".to_string())],
        initialization_options: Some(serde_json::json!({"a": 1})),
        ready: ReadySignal::Progress,
    }
}

#[test]
fn two_commands_are_equal_when_they_start_the_same_server_whatever_their_ready_signal() {
    let base = command("server");
    let mut other_signal = command("server");
    other_signal.ready = ReadySignal::HoldsQuestions;
    assert_eq!(base, other_signal);
    for (name, changed) in [
        (
            "program",
            ServerCommand {
                program: "other".into(),
                ..command("server")
            },
        ),
        (
            "args",
            ServerCommand {
                args: Vec::new(),
                ..command("server")
            },
        ),
        (
            "env",
            ServerCommand {
                env: Vec::new(),
                ..command("server")
            },
        ),
        (
            "options",
            ServerCommand {
                initialization_options: None,
                ..command("server")
            },
        ),
    ] {
        assert_ne!(base, changed, "{name}");
    }
}

#[test]
fn a_program_runs_when_its_version_flag_exits_successfully() {
    assert!(runs("true"));
    assert!(!runs("false"));
    assert!(!runs("no-such-program-for-unit-tests"));
}

#[test]
fn only_python_keeps_its_initialization_options() {
    let options = Some(serde_json::json!({"python": {"analysis": {}}}));
    assert_eq!(
        server_initialization_options("python", options.clone()),
        options
    );
    assert_eq!(server_initialization_options("python", None), None);
    assert_eq!(server_initialization_options("go", options), None);
}

#[test]
fn a_server_is_offered_only_for_a_language_the_node_has_installed() {
    assert_eq!(server_command("not-a-language"), None);
    assert_eq!(server_command(""), None);
    let workspace = tempfile::tempdir().unwrap();
    for engine in [
        "rust",
        "go",
        "cpp",
        "python",
        "typescript",
        "swift",
        "java",
        "kotlin",
        "csharp",
        "php",
        "ruby",
        "dart",
        "zig",
        "elixir",
        "scala",
        "lua",
        "haskell",
        "ocaml",
        "clojure",
        "julia",
        "shell",
        "r",
        "erlang",
        "fsharp",
        "perl",
        "solidity",
        "nim",
        "d",
        "fortran",
        "sql",
        "graphql",
        "protobuf",
        "crystal",
        "groovy",
        "ada",
        "v",
        "racket",
        "terraform",
        "nix",
        "markdown",
        "yaml",
        "toml",
        "json",
        "html",
        "css",
        "dockerfile",
        "svelte",
        "vue",
        "assembly",
    ] {
        #[allow(clippy::manual_flatten)]
        for found in [
            server_command(engine),
            server_command_for_workspace(engine, workspace.path()),
        ] {
            // Offered only with a program that exists on this node.
            if let Some(found) = found {
                if engine == "rust" {
                    assert_eq!(found.program, "rust-analyzer");
                    assert!(runs(&found.program), "{engine}");
                } else {
                    assert!(
                        Path::new(&found.program).is_file()
                            || prod_code_engine_generic::which_bin(&found.program).is_ok(),
                        "{engine}: {}",
                        found.program
                    );
                }
            }
        }
    }
}
