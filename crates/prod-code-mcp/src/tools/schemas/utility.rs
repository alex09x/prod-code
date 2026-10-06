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

pub fn utility_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "code_report_issue".to_string(),
            description: "Report a bug in prod-code ITSELF (not in the code you are working on) as a GitHub issue in alex09x/prod-code: a wrong or empty answer from a code_* tool, a hang, a crash, an error that does not say what to do, or a missing capability you needed. Use it as soon as you are sure the tool is at fault, then carry on with your task. Give a searchable `title` (what went wrong, in which tool, for which language) and a `body` with the exact tool call or command and its arguments, what it returned (paste the output), what you expected, and how to reproduce it. LAN addresses, the host name and home-directory paths are removed before anything is sent, and the client and node versions are added. Issues with a similar title, open or closed, are listed first and nothing is filed unless `force: true`; comment on an open one that is the same problem, and check whether a closed one is fixed in a newer release. When reproducing needs private details (host names, addresses, internal paths or logs), keep them in your own private record first and pass its id as `private_ref`; never put them in the body. Pass `labels`: one type (bug, enhancement, documentation, perf; bug when none is given) and the areas the issue is about (gateway, client, mcp, cluster, worktree, infra, test). `dry_run: true` shows the issue without filing it."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "title": { "type": "string", "description": "What went wrong, where: e.g. `code_references returns nothing for a Go struct field`" },
                    "body": { "type": "string", "description": "The call and its arguments, what came back (output), what was expected, how to reproduce" },
                    "private_ref": { "type": "string", "description": "Id of a private record of details that cannot be public (hosts, addresses, internal paths or logs), kept by you elsewhere; the issue names it instead of the details" },
                    "labels": { "type": "array", "items": { "type": "string" }, "description": "One type (bug, enhancement, documentation, perf; bug when none is given) and the areas it is about (gateway: the server daemon and its engines, client: the CLI and sync, mcp: the MCP tools, cluster: placement across nodes, worktree: worktree copies, infra: node setup and deploys, test: tests and coverage)" },
                    "force": { "type": "boolean", "description": "File even when similar issues exist" },
                    "dry_run": { "type": "boolean", "description": "Show the scrubbed issue without filing it" }
                },
                "required": ["title", "body"]
            }),
        },
        McpTool {
            name: "code_status".to_string(),
            description: "Check remote prod-code gateway health, memory RSS, active engines, loaded workspaces, and network RTT."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        },
        McpTool {
            name: "code_sync".to_string(),
            description: "Push local modified workspace files to the remote server over 10G LAN for immediate compilation and analysis."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Optional specific subfolder or file to sync. Defaults to entire workspace root."
                    }
                }
            }),
        },
        McpTool {
            name: "code_dependencies".to_string(),
            description: "Analyze architectural dependencies, calculate afferent (Ca) and efferent (Ce) coupling metrics, instability index (Ce / (Ca + Ce)), and detect circular dependency cycles (e.g. A -> B -> C -> A) using Tarjan's algorithm. Supports `scope: \"crates\"` (Cargo / Go module manifests) and `scope: \"modules\"` (Rust, Go, Python, TS/JS imports)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["crates", "modules"],
                        "description": "Granularity: 'crates' (default, workspace crates/packages) or 'modules' (source file module imports)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional subdirectory to narrow the dependency analysis scope"
                    }
                }
            }),
        },
        McpTool {
            name: "code_find_duplicates".to_string(),
            description: "Scan workspace source files for code duplications and copy-paste clones. Detects Type-1 (exact token clones) and Type-2 (parameterized clones with renamed variables/differing literals), grouping occurrences and generating recommendations to fold into shared functions."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "min_lines": {
                        "type": "integer",
                        "description": "Minimum consecutive duplicated lines to report (default 6)"
                    },
                    "parameterized": {
                        "type": "boolean",
                        "description": "Detect Type-2 parameterized clones where identifier names and literals vary (default true)"
                    },
                    "max_groups": {
                        "type": "integer",
                        "description": "Maximum number of clone groups to return (default 20)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional directory path to narrow the clone search"
                    }
                }
            }),
        },
        McpTool {
            name: "code_structural_search".to_string(),
            description: "Polyglot Structural AST Pattern Search across Rust, Go, TypeScript/JS, Python, C/C++, and Swift. Matches syntax trees regardless of formatting, whitespace, or variable names using metavariables (`$name`), returning exact match locations and bound expression snippets."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Structural AST pattern with metavariables, e.g. `$a.unwrap()`, `errors.Wrap($err, $msg)`, or `if ($x == nil) { return $y }`"
                    },
                    "path": {
                        "type": "string",
                        "description": "Optional file or directory path to search within"
                    }
                },
                "required": ["pattern"]
            }),
        },
        McpTool {
            name: "code_propose_expression".to_string(),
            description: "Type-Directed Expression Synthesis: synthesizes valid in-scope expressions, borrow/deref conversions, String conversions, and 1-2 hop accessor chains that evaluate to a requested target type (e.g. `AccountId`, `String`, `Option<T>`), ranked by confidence to prevent hallucinated API calls."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Source file path"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number where the expression is needed"
                    },
                    "target_type": {
                        "type": "string",
                        "description": "The expected target type to synthesize, e.g. 'String', '&str', 'u64', 'Option<T>'"
                    }
                },
                "required": ["path", "line", "target_type"]
            }),
        },
    ]
}
