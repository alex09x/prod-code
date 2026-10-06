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

pub fn navigation_tools() -> Vec<McpTool> {
    vec![
        McpTool {
            name: "code_callers".to_string(),
            description: "Incoming call hierarchy: every function/method in the workspace that calls the given function, with the call sites. With `depth`, their callers too, as a tree. Name the function with `symbol` (e.g. `Metrics::record`); path/line/character is the alternative. Semantic (resolved through the analyzer), not a text search."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" },
                    "depth": { "type": "integer", "description": "Levels to walk (default 1, the direct ones; at most 6). Deeper levels come as an indented tree; a function already shown is marked instead of expanded again" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_callees".to_string(),
            description: "Outgoing call hierarchy: every function/method the given function calls, with the call sites. With `depth`, what those call too, as a tree. Name it with `symbol`, or give path/line/character."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" },
                    "depth": { "type": "integer", "description": "Levels to walk (default 1, the direct ones; at most 6). Deeper levels come as an indented tree; a function already shown is marked instead of expanded again" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_implementations".to_string(),
            description: "All implementations of a trait/interface/abstract class (or the impl blocks of a type), as locations. Give `symbol` (its name) or a file position (path + 1-based line/column)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_supertypes".to_string(),
            description: "What a type implements, or what a trait requires: the upward half of the type hierarchy (`code_implementations` is the downward half). For a Rust type, the traits it implements, derived or written as impl blocks, each with the position of the impl or the derive; inherent impls are not listed. For a Rust trait, its supertraits. Other languages ask their server's own type hierarchy (clangd and gopls answer it) and say so when it has none. Give `symbol` or a file position; `depth` (at most 6) walks the hierarchy transitively."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" },
                    "depth": { "type": "integer", "description": "Hierarchy depth to traverse (default 1, max 6)" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_definition".to_string(),
            description: "Find where a symbol (function, struct, type, variable, module) is defined. Give `symbol` (its name, e.g. `WorkspaceSymbol` or `Metrics::record`) or a file position (path + 1-based line/column) of a use of it. Returns the definition's code (a function with its body, a type with its fields, with doc comments), numbered (at most 300 lines), by default so there is no need to read the file or grep for it. Pass `body: false` to return only the location."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "body": {
                        "type": "boolean",
                        "description": "Return the definition's code, numbered (at most 300 lines); default true. Pass false to return only the location"
                    },
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_references".to_string(),
            description: "Find every reference to a symbol across the workspace. Give `symbol` (its name) or a file position (path + 1-based line/column). With `also_in` and `symbol`, other checkouts are searched too, each resolving the name itself."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    },
                    "include_declarations": {
                        "type": "boolean",
                        "description": "Include the declaration site in the reference list (default: false)"
                    },
                    "also_in": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Directories of other checkouts to search as well (with `symbol`), e.g. two services that use the same dependency's type"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Optional display truncation limit (default: 0 for all). Caps the number of reference lines rendered in the output text to keep agent context windows bounded for heavily-referenced symbols. Internal AST refactoring tools always receive the complete reference set."
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_source".to_string(),
            description: "Read a source file from the local workspace or from the gateway host. Accepts a workspace-relative path or an absolute path/file URI inside the local checkout; external absolute paths such as standard library sources (Rust std, Go GOROOT, Swift frameworks, system C++ headers), dependency registries and caches (Cargo registry/git, Go pkg/mod, node_modules, npm/bun/pnpm/yarn, Python uv/poetry/pipx/virtualenv wheels), and SDK headers are read from the gateway. Optionally a window of lines around one line."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Workspace-relative path, absolute path or file:// URI inside the local checkout, or an external absolute path/file URI on the gateway (as returned by code_definition)" },
                    "line": { "type": "integer", "description": "1-based line to centre on; omitted: the whole file (up to 2 MiB)" },
                    "context": { "type": "integer", "description": "Lines of context around `line` (default 30)" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_outline".to_string(),
            description: "Extract the structural symbol outline (functions, structs, enums, traits, classes, methods, fields) with line numbers from a source file or directory (a package: its files, in name order). Local variables are left out unless `include_locals` is set. Narrow a large package with `kinds` and `exported_only`; a directory's outline stops at a byte budget (`max_bytes`) and says which files it did not reach."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File or directory path (relative to workspace or absolute)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum nesting depth to list, 1 = top-level items only (default: 3)"
                    },
                    "include_locals": {
                        "type": "boolean",
                        "description": "Also list local variables and bindings inside function bodies (default: false)"
                    },
                    "kinds": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Only these kinds, as the outline names them: function, method, struct, class, interface, enum, field, property, constant, variable, module (default: all)"
                    },
                    "exported_only": {
                        "type": "boolean",
                        "description": "Only what the language exports: Go's capitalised names, Rust's `pub`, Swift's `public`/`open`, TypeScript's `export`, Python's names without a leading underscore (default: false)"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Most bytes of outline listed; the files after it are named, not outlined. Default 40000 for a directory, none for a file; 0 = no budget"
                    },
                    "max_items": {
                        "type": "integer",
                        "description": "Most symbols listed (default: no limit)"
                    }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_symbols".to_string(),
            description: "Search the workspace symbol index by name (fuzzy, analyzer-backed): functions, types, methods, constants across every file, with file:line:col and the enclosing item. Use it to locate a symbol, or pass `symbol` directly to code_callers / code_references / code_hover and the other position tools."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Symbol name or prefix (e.g. `record`, `Metrics`); case-insensitive fuzzy match" },
                    "limit": { "type": "integer", "description": "Maximum hits (default 30)" },
                    "path": { "type": "string", "description": "A file or directory inside a nested project to search that project instead of the root" }
                },
                "required": ["query"]
            }),
        },
        McpTool {
            name: "code_hover".to_string(),
            description: "Inspect the type signature, docstring and documentation of a symbol. Give `symbol` (its name, e.g. `narrow_scope` or `Metrics::record`) or a file position (path + 1-based line/column)."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_type_at".to_string(),
            description: "Alias for code_hover: inspect the type and documentation of a code expression."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path"
                    },
                    "line": {
                        "type": "integer",
                        "description": "1-based line number"
                    },
                    "character": {
                        "type": "integer",
                        "description": "1-based column/character number"
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
    ]
}
