/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::COMPILE_DESCRIPTION;
use crate::protocol::McpTool;

pub fn analysis_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "code_impact".to_string(),
            description: "Blast radius of the uncommitted changes (or of the commits since a base ref): the functions the diff touches, every function that calls them (transitively, through the analyzer's call hierarchy) and the tests among those callers (and changed tests themselves), plus the exact test command that runs only the affected tests. What it could not establish (a deleted file, a failed or unreadable analyzer answer, callers left beyond the depth limit) is listed as incomplete analysis: run the whole suite then."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "base": { "type": "string", "description": "Git ref to diff against (default: the working tree against HEAD)" },
                    "depth": { "type": "integer", "description": "How many caller levels to follow (default 4)" }
                }
            }),
        },
        McpTool {
            name: "code_diagnose_failure".to_string(),
            description: "Run the tests (optionally one filter) on the gateway and, for every failure, return a dossier: the failing test, panic line, expression, runtime values, suspects from recent changes, failure output, the source around each location it mentions, the enclosing function and its callers, and the working-tree diff of that file. Supports polyglot assertion evidence across Rust, Node/TS, Python (pytest, unittest), Go (testify, got/want), Swift (XCTest, swift-testing), and C++ (GoogleTest, Catch2). Returns human-readable text by default, or structured JSON when `json: true`. One call instead of test → grep → read → blame."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Test name filter, as for code_test" },
                    "timeout_secs": { "type": "integer", "description": "Kill the run after this many seconds (default 3600)" },
                    "path": { "type": "string", "description": "Optional file path to hint language or narrow scope" },
                    "json": { "type": "boolean", "description": "Return structured JSON failure dossier instead of human-readable text" }
                }
            }),
        },
        McpTool {
            name: "code_diagnostics".to_string(),
            description: "Analyzer diagnostics for one file, computed in memory without a build: syntax errors, unresolved names, type mismatches, unused items. Milliseconds, not a cargo/tsc run."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_validate_edit".to_string(),
            description: "Check a proposed new content for a file BEFORE writing it: the analyzer sees the proposed text as the document and reports errors and warnings. Nothing is written anywhere. Use it to catch hallucinated APIs, type errors and unresolved imports before touching the checkout. `stream_chunks` enables on-the-fly incremental validation during agent code generation, intercepting hallucinations before turn conclusion (Roadmap 7.7). For Rust the analyzer runs no borrow checker; `compile: true` or `borrow_check: true` has the compiler and borrow checker judge the text too."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute); may be a new file" },
                    "new_text": { "type": "string", "description": "The complete proposed content of the file" },
                    "chunk": {
                        "type": "string",
                        "description": "Incremental code chunk to feed to an active streaming validation session (Roadmap 7.7)"
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Session identifier for stateful incremental stream validation (default: 'default')"
                    },
                    "close": {
                        "type": "boolean",
                        "description": "Mark the streaming session as closed/final on this chunk, triggering final validation and borrow-checking"
                    },
                    "reset": {
                        "type": "boolean",
                        "description": "Reset session buffer before feeding this chunk"
                    },
                    "stream_chunks": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Streamed code chunks to validate incrementally during agent generation, intercepting hallucinations at delimiter checkpoints before turn completion (Roadmap 7.7)"
                    },
                    "compile": { "type": "boolean", "description": COMPILE_DESCRIPTION },
                    "borrow_check": { "type": "boolean", "description": "Enforce borrow checker and full compiler verification in a shadow workspace (Roadmap 7.7)" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_validate_edits".to_string(),
            description: "Check several proposed file contents TOGETHER before writing any of them: all edits are placed in one private analyzer overlay, then diagnostics are reported per file, so a change in one file is judged against the proposed state of the others (a changed signature and its updated callers). The change can be given as whole files (`edits`), as a proposed unified diff (`diff`: each hunk is applied to CURRENT on-disk contents, not Git HEAD, where it says or where its old lines moved to; submit it before applying the edits, and a hunk that fits nowhere is refused by number), or as an LSP WorkspaceEdit (`workspace_edit`). If edits are already written, provide their complete current contents in `edits` instead. `also_check` lists unchanged files that might break (callers of the edited symbols). Nothing is written anywhere."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "edits": {
                        "type": "array",
                        "description": "Proposed complete contents, one entry per file",
                        "items": {
                            "type": "object",
                            "properties": {
                                "path": { "type": "string", "description": "File path (relative to workspace or absolute); may be a new file" },
                                "new_text": { "type": "string", "description": "The complete proposed content of the file" }
                            },
                            "required": ["path", "new_text"]
                        }
                    },
                    "diff": { "type": "string", "description": "A proposed unified diff against CURRENT on-disk files, not Git HEAD. Submit before applying it; for already-written edits, use complete contents in `edits`" },
                    "workspace_edit": { "type": "object", "description": "The change as an LSP WorkspaceEdit (`changes` or `documentChanges`), instead of `edits`" },
                    "also_check": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Unchanged files to diagnose against the proposed edits (optional)"
                    },
                    "compile": { "type": "boolean", "description": COMPILE_DESCRIPTION },
                    "borrow_check": { "type": "boolean", "description": "Enforce borrow checker and full compiler verification in a shadow workspace (Roadmap 7.7)" }
                },
                "anyOf": [
                    { "required": ["edits"] },
                    { "required": ["diff"] },
                    { "required": ["workspace_edit"] }
                ]
            }),
        },
        McpTool {
            name: "code_shadow_run".to_string(),
            description: "Try several candidate edits AGAINST A COMMAND (usually the tests) without touching the checkout. Each hypothesis is a name plus complete proposed file contents; the gateway runs `argv` once per hypothesis in a private shadow of the workspace (on Linux an overlay mounted at the workspace's own path, so warm build caches stay valid and hypotheses run in parallel). Returns every hypothesis's exit code, test counts and output tail, ranks them (passed, fewest failures, most passed tests, smallest diff) and prints the winner's unified diff; `apply: true` writes the winner into the checkout. One hypothesis is a dry run of a fix; a hypothesis without edits is the baseline."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "hypotheses": {
                        "type": "array",
                        "description": "Candidates to compare, each a complete set of proposed file contents",
                        "items": {
                            "type": "object",
                            "properties": {
                                "name": { "type": "string", "description": "Short label, unique within the run" },
                                "edits": {
                                    "type": "array",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "path": { "type": "string", "description": "File path (relative to workspace or absolute); may be a new file" },
                                            "new_text": { "type": "string", "description": "The complete proposed content of the file" }
                                        },
                                        "required": ["path", "new_text"]
                                    }
                                },
                                "delete": { "type": "array", "items": { "type": "string" }, "description": "Files this hypothesis removes (optional)" }
                            },
                            "required": ["name"]
                        }
                    },
                    "argv": { "type": "array", "items": { "type": "string" }, "description": "Command run once per hypothesis, e.g. [\"cargo\", \"test\", \"-p\", \"my-crate\"]" },
                    "cwd": { "type": "string", "description": "Directory inside the workspace to run in (relative to the workspace root); default: the root" },
                    "apply": { "type": "boolean", "description": "Write the winning hypothesis into the checkout (default false)" },
                    "timeout_secs": { "type": "integer", "description": "Kill a hypothesis after this many seconds (default 3600)" },
                    "parallel": { "type": "integer", "description": "Hypotheses run at once (default: server cores / 8)" },
                    "tail_bytes": { "type": "integer", "description": "Output kept per hypothesis (default 16384)" },
                    "in_memory": { "type": "boolean", "description": "Run hypotheses in a lightweight RAM-backed (/dev/shm) in-memory overlay shadow root (Roadmap 7.4)" },
                    "ram": { "type": "boolean", "description": "Alias for in_memory" }
                },
                "required": ["hypotheses", "argv"]
            }),
        },
        McpTool {
            name: "code_slice".to_string(),
            description: "Return only the code a symbol depends on, instead of the files it lives in. Starting from the symbol, the analyzer's own edges are followed: the functions it calls, and the types, constants and traits its body mentions, each returned as its whole declaration with its file and line range. Supports intra-function backward data-flow and control-dependency slicing inside function bodies (Roadmap 7.3) with explicit completeness contract (Complete, Bounded, Incomplete) via `dataflow: true`. `depth` bounds how far the walk goes (default 2), `max_bytes` bounds the result. Use it to read an unfamiliar function without opening four files, and to hand a model the relevant tenth of a codebase rather than the whole of it. Names that resolve outside the workspace (std, dependencies) are listed, not expanded."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Name of the symbol to slice from (`Metrics::record`, `pkg.Func`); or give path/line/character" },
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the symbol" },
                    "character": { "type": "integer", "description": "1-based column of the symbol" },
                    "depth": { "type": "integer", "description": "How many edges to follow from the seed (default 2, 0 returns the seed alone)" },
                    "max_bytes": { "type": "integer", "description": "Stop once the slice reaches this many bytes (default 24576)" },
                    "dataflow": { "type": "boolean", "description": "When true, perform intra-function backward data-flow and control-dependency slicing inside the function body (Roadmap 7.3)" },
                    "target_line": { "type": "integer", "description": "Target line for intra-function data-flow slicing criterion (1-based, defaults to cursor line or return statement)" },
                    "target_var": { "type": "string", "description": "Target variable name for intra-function data-flow slicing criterion" }
                },
                "required": []
            }),
        },
        McpTool {
            name: "code_search".to_string(),
            description: "Find code by what it does when you do not know what it is called. The gateway indexes every declaration in the workspace together with the doc comment above it, and ranks them against your question's words (BM25 over name, container, signature and doc, name weighted highest). Ask it the way you would ask a colleague: \"where do we decide which node runs a workspace\". Returns declarations with file:line, signature and the doc sentence that matched, best first. When the gateway has its embedding model, every declaration is also ranked by meaning (a small sentence-embedding model, computed in the background after the index is built) and the two rankings are fused, so a question sharing no words with the code still finds it; the result says how far the embedding has got, or that the search is lexical only. `code_symbols` remains the way to look up a name you already know."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What the code does, in words (`how do we retry a dropped websocket`)" },
                    "limit": { "type": "integer", "description": "Hits to return (default 10)" },
                    "path": { "type": "string", "description": "Restrict to declarations under this directory (relative to the workspace root)" }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "code_codemod".to_string(),
            description: "Structural search and replace across the whole workspace, on the syntax tree rather than on text. The rule is rust-analyzer's own SSR syntax, `pattern ==>> replacement`, where `$name` is a placeholder the match binds: `$a.unwrap() ==>> $a.expect(\"invariant\")`, `Foo::new($a, $b) ==>> Foo::builder().a($a).b($b).build()`. A call split over three lines still matches, a comment that looks like the pattern does not, and paths are resolved rather than compared as strings. Returns a unified diff of what it would change; `apply: true` writes it into the checkout. Rust only, and not interactive: the search resolves usages across the workspace, so a call takes tens of seconds to minutes."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "rule": { "type": "string", "description": "`pattern ==>> replacement` with `$name` placeholders" },
                    "path": { "type": "string", "description": "Restrict the edits to this one file, which also resolves the paths the pattern mentions. It does not make the call faster: the search resolves usages across the workspace either way, which takes tens of seconds to minutes on a warm engine." },
                    "apply": { "type": "boolean", "description": "Write the edits into the checkout (default false: report only)" }
                },
                "required": ["rule"]
            }),
        },
        McpTool {
            name: "code_generate_fixture".to_string(),
            description: "Build a compile-ready value or test mock for a type from the declaration the analyzer resolves the name to, across Rust, Go, TypeScript, Python, C++, and Swift. Fields are filled with appropriate types, types declared in this workspace are built field by field down to `depth`, and anything deeper or foreign falls back to defaults. With `randomized: true`, generates realistic non-zero test dummy data (tokens, realistic numbers, dates, non-empty collections). With `mock: true`, generates a mock implementation with call tracking for interfaces, traits, or protocols. With `verify` (default true) the fixture is type-checked in an in-memory overlay. With `builder: true` (Rust only), generates a typed builder for a named struct."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "The type to build (`Config`, `SliceReport`, `UserService`)" },
                    "path": { "type": "string", "description": "The file that declares it, when the name is ambiguous (a re-export makes a type resolve twice)" },
                    "depth": { "type": "integer", "description": "How deep to build nested workspace types before falling back to defaults (default 2)" },
                    "builder": { "type": "boolean", "description": "Generate a typed Rust builder instead of a value (default false); omit depth" },
                    "builder_name": { "type": "string", "description": "Generated builder name (default TypeBuilder); requires builder=true" },
                    "randomized": { "type": "boolean", "description": "Generate realistic non-zero dummy test data instead of default empty/zero values" },
                    "mock": { "type": "boolean", "description": "Generate a test mock implementation with call tracking for an interface, trait, protocol or struct" },
                    "language": { "type": "string", "description": "Explicit language override: 'rust', 'go', 'typescript', 'python', 'cpp', 'swift'" },
                    "verify": { "type": "boolean", "description": "Type-check the fixture before returning it (default true)" }
                },
                "required": ["symbol"]
            }),
        },
        McpTool {
            name: "code_dead_code".to_string(),
            description: "Unreferenced functions, methods and types across the checkout, found through the analyzer's references (not text search). Exported/public symbols are counted separately unless include_exported is set; tests and entry points are skipped. A symbol whose references the analyzer failed to answer for is listed as unverified, never as dead."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "include_exported": { "type": "boolean", "description": "Also list exported / public symbols nothing in the checkout uses" },
                    "reachability": { "type": "boolean", "description": "Perform whole-program graph reachability analysis from entry points (main, public APIs, tests, route handlers) to detect unreachable functions, types, and circular dead cycles" },
                    "max_files": { "type": "integer", "description": "Stop after this many source files (default 400)" }
                }
            }),
        },
        McpTool {
            name: "code_prune_orphans".to_string(),
            description: "Remove every orphan the dead-code scan finds (unreferenced, not exported, not reachable through a trait) with the analyzer's safe delete, all in one edit. The whole result is type-checked in one overlay before anything is written. Supports generating Git commit patches (git_patch: true) and creating Git commits (commit: true). Deletions that overlap another are left for the next run, and so is what these removals orphan: run it again until it finds nothing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_files": { "type": "integer", "description": "Stop after this many source files (default 400)" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report what would be removed and the type check)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" },
                    "reachability": { "type": "boolean", "description": "Prune unreachable functions, types, and circular dead cycles detected by whole-program reachability analysis" },
                    "git_patch": { "type": "boolean", "description": "Output a formatted Git commit patch compatible with git apply / git am" },
                    "commit": { "type": "boolean", "description": "Create a Git commit after applying the pruned changes (implies apply: true)" }
                }
            }),
        },
    ]
}
