/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::protocol::McpTool;

pub fn execution_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "code_exec".to_string(),
            description: "Run a build, test, lint or format command on the remote gateway inside this workspace's server copy (warm per-worktree caches, 32-core server). The checkout is synced first; files the command changes (formatters, generators, lockfiles) are written back. Returns the exit code and the tail of the combined output."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "argv": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Command and arguments, e.g. [\"cargo\", \"test\", \"-p\", \"my-crate\"]"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Kill the command after this many seconds (default 3600)"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "Directory inside the workspace to run in (relative to the workspace root); default: the root"
                    },
                    "tail_bytes": {
                        "type": "integer",
                        "description": "How much of the output tail to return (default 16384)"
                    }
                },
                "required": ["argv"]
            }),
        },
        McpTool {
            name: "code_check".to_string(),
            description: "Compile-check the whole workspace on the remote gateway (cargo check / go build) and return structured compiler errors and warnings with file:line:col. Nothing runs on the local machine. `fix: true` (Rust) applies every fix the compiler marks machine-applicable to the checkout in one edit, then checks again and reports what was fixed, what was skipped and why, and what is left."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for the command, e.g. {\"RUST_BACKTRACE\": \"1\"}" },
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "fix": { "type": "boolean", "description": "Rust: apply the compiler's machine-applicable fixes, then check again (default false)" },
                    "path": { "type": "string", "description": "Narrow the run: a file or directory inside the project (runs only its Cargo crate / Go package tree / pytest path), a crate name (`prod-code-gateway`), or a nested project of another language (a SwiftPM package in a Rust repo)" }
                }
            }),
        },
        McpTool {
            name: "code_lint".to_string(),
            description: "Lint the whole workspace on the remote gateway (cargo clippy -D warnings; golangci-lint, or go vet on a node without it; ruff; eslint / biome; clang-tidy) and return structured findings with file:line:col. `fix: true` (Rust) applies every fix clippy and rustc mark machine-applicable, then lints again."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for the command, e.g. {\"RUST_BACKTRACE\": \"1\"}" },
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "fix": { "type": "boolean", "description": "Apply the fixes, then lint again (default false): Rust takes clippy's machine-applicable suggestions; Python, TypeScript and C++ run the linter's own fix mode (`ruff check --fix`, `eslint --fix` / `biome lint --write`, `clang-tidy -fix`) on the node and bring the rewritten files back; Go runs `golangci-lint run --fix` when the project has a golangci config, and has no fix mode otherwise" },
                    "path": { "type": "string", "description": "Narrow the run: a file or directory inside the project (runs only its Cargo crate / Go package tree / pytest path), a crate name (`prod-code-gateway`), or a nested project of another language (a SwiftPM package in a Rust repo)" }
                }
            }),
        },
        McpTool {
            name: "code_benchmarks".to_string(),
            description: "Run the project's benchmarks on the remote gateway (cargo bench / go test -bench) and return each result parsed: name, estimate and range (criterion's interval, libtest's +/-, Go's ns/op). `filter` selects benchmarks by name."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Benchmark name filter (cargo bench FILTER / go test -bench PATTERN)" },
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for the command, e.g. {\"RUST_BACKTRACE\": \"1\"}" },
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "path": { "type": "string", "description": "Narrow the run to a crate, package or nested project, as for code_test" }
                }
            }),
        },
        McpTool {
            name: "code_test".to_string(),
            description: "Run tests on the remote gateway (cargo test / go test -json / pytest / vitest / …), optionally filtered by test name, and return pass/fail counts plus the output of each failed test. `path` narrows the run to the Cargo crate, Go package tree or pytest directory/file containing it."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Test name filter (cargo test TESTNAME / go test -run)" },
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for the command, e.g. {\"RUST_BACKTRACE\": \"1\"}" },
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "path": { "type": "string", "description": "Narrow the run: a file or directory inside the project (runs only its Cargo crate / Go package tree / pytest path), a crate name (`prod-code-gateway`), or a nested project of another language (a SwiftPM package in a Rust repo)" }
                }
            }),
        },
        McpTool {
            name: "code_assists".to_string(),
            description: "List the code actions the remote analyzer offers at a 1-based position or selection: inline, extract function/variable/constant, introduce parameter, generate impl/getters, convert/rewrite forms, quick fixes. Each entry has an id (and optional subtype) for code_assist."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line" },
                    "character": { "type": "integer", "description": "1-based column" },
                    "end_line": { "type": "integer", "description": "1-based end line of a selection (optional)" },
                    "end_character": { "type": "integer", "description": "1-based end column of a selection (optional)" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_assist".to_string(),
            description: "Apply one code action from code_assists (by id, optional subtype) at the same position or selection; the resulting edits are written into the checkout and the changed paths reported."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line" },
                    "character": { "type": "integer", "description": "1-based column" },
                    "id": { "type": "string", "description": "Assist id from code_assists, e.g. extract_function" },
                    "subtype": { "type": "integer", "description": "Assist subtype from code_assists when several share an id" },
                    "end_line": { "type": "integer", "description": "1-based end line of a selection (optional)" },
                    "end_character": { "type": "integer", "description": "1-based end column of a selection (optional)" }
                },
                "required": ["path", "line", "character", "id"]
            }),
        },
    ]
}
