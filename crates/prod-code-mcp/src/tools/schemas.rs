/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! MCP tool schemas and definitions.

use crate::protocol::McpTool;

pub const POSITION_ARGUMENTS: [&str; 4] = ["line", "character", "end_line", "end_character"];

pub const COMPILE_DESCRIPTION: &str = "Also run the project's check command (`cargo check` for Rust,      `go build`, `tsc`, ...) on the proposed text in a private shadow copy on the node, and report      the compiler's errors: the analyzer does not check everything the compiler does (rust-analyzer      runs no borrow checker, so a reference to a local, E0515, or a use after a move, E0382, passes      without it; nor does it report a private function of another crate, E0603). Slower: a build,      warm on the node";

/// Tools that accept `symbol` in place of `path`/`line`/`character`.
pub const SYMBOL_ADDRESSABLE: &[&str] = &[
    "code_slice",
    "code_change_signature",
    "code_move",
    "code_introduce_parameter_object",
    "code_migrate_type",
    "code_encapsulate_field",
    "code_wrap_return",
    "code_make_static",
    "code_convert_to_method",
    "code_invert_boolean",
    "code_generify",
    "code_definition",
    "code_references",
    "code_hover",
    "code_type_at",
    "code_callers",
    "code_callees",
    "code_implementations",
    "code_supertypes",
    "code_rename",
    "code_safe_delete",
    "code_assists",
    "code_assist",
    "code_replace_constructor_with_factory",
    "code_replace_constructor_with_builder",
    "code_replace_constructor",
    "code_pull_up",
    "code_push_down",
    "code_replace_inheritance_with_delegation",
    "code_replace_conditional_with_polymorphism",
    "code_extract_interface",
];

pub fn build_tools_raw() -> Vec<McpTool> {
    let mut tools = vec![
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
            name: "code_safe_delete".to_string(),
            description: "Delete the item at a 1-based declaration-name position only when the analyzer proves it unreferenced. Rust items retain analyzer safe-delete behavior, and Rust parameters retain the code_change_signature rewrite. Go retains its narrow compiler-verified function and receiver-method support. TypeScript writes only an ordinary private ASCII-named, non-generic, non-async, non-generator top-level function with a body in a contained ES module and simple tsconfig include graph: native declarations and references must exactly match current source, and the complete proposal must pass tsc --noEmit in an isolated remote shadow. JavaScript, TSX, exports, ambient/overload/decorator forms, dynamic evaluation, linked/generated sources and complex configuration graphs are refused. Force bypasses none of the Go or TypeScript checks. Imports are not cleaned automatically."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the item's name" },
                    "character": { "type": "integer", "description": "1-based column of the item's name" },
                    "force": { "type": "boolean", "description": "Rust parameter compatibility option; it never bypasses Go or TypeScript safe-delete checks" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_rename".to_string(),
            description: "Preferred tool for workspace-wide renames: Use this tool when renaming any symbol (type, function, field, variable, module) across the workspace instead of manual multi-file text edits. Driven by the remote analyzer, the result is type-checked in one overlay before anything is written, and a rename that does not compile — typically a new name already declared in the same scope — is refused with the errors unless `force` is given. Rewrites every affected file in the checkout (and renames module files) and reports what changed. Fall back to manual edits if the symbol or files are unsupported."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line number" },
                    "character": { "type": "integer", "description": "1-based column/character number" },
                    "new_name": { "type": "string", "description": "New identifier" },
                    "accessors": { "type": "boolean", "description": "At a field: also rename the methods of its struct's `impl` blocks named after it (`f`, `get_f`, `set_f`, `f_mut`), with every call, merged into one change" },
                    "comments": { "type": "boolean", "description": "Also replace the old name where it stands as a whole word in comments, and its snake_case form in the names of test functions (`order_total_rounds` -> `trade_total_rounds`), in every file the rename touches; one change, type-checked. Not with a rename that moves files" },
                    "force": { "type": "boolean", "description": "Write the rename even when the result does not compile" }
                },
                "required": ["path", "line", "character", "new_name"]
            }),
        },
        McpTool {
            name: "code_schema_rename".to_string(),
            description: "Rename a schema field across every language that spells it: `order_id` in the .proto and in Rust, `OrderID` with a `json:\"order_id\"` tag in Go, `orderId` in TypeScript, the column in the SQL. All spellings (snake, camel, Pascal, Go's initialism form, SCREAMING, kebab) are found by a whole-word scan — that is discovery, not editing. Then every identifier is renamed by the analyzer of its own sub-project, so the change follows the symbol into files the scan never looked at; only what no analyzer owns (schema files, and the name inside string literals such as a json tag or an SQL query) is edited textually, at the positions that were found. Identifiers in comments are reported, not rewritten. OpenAPI documents and GraphQL schemas are read for their structure: the field is rewritten where it is a key or a whole value (OpenAPI) or a name (GraphQL), and a description or comment that mentions it is listed instead. `repos` adds more repositories (a frontend next to this backend) to the same change: each is planned and checked by its own analyzers, and `apply` writes all of them or none. The result is type-checked per project, and `apply` refuses to write a rename that does not compile. Nothing is written without `apply`."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "field": { "type": "string", "description": "The field as the schema spells it (`order_id`)" },
                    "to": { "type": "string", "description": "What it becomes (`trade_id`); spelled per language automatically" },
                    "path": { "type": "string", "description": "Only look under this directory (default: the whole workspace)" },
                    "repos": { "type": "array", "items": { "type": "string" }, "description": "More repositories to rename in as one change (`../frontend`): paths, absolute or relative to this workspace. Each is planned and checked by its own analyzers, and `apply` writes all of them or none. Not with `path` or `verify`" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the rename (default false: report the diff and the checks only)" },
                    "force": { "type": "boolean", "description": "Allow a short name, a large number of occurrences, and writing a result that does not compile" },
                    "workspace_edit": { "type": "boolean", "description": "Emit standard LSP WorkspaceEdit (documentChanges) JSON payload" }
                },
                "required": ["field", "to"]
            }),
        },
        McpTool {
            name: "code_encapsulate_field".to_string(),
            description: "Encapsulate a field with idiomatic getters and setters, rewriting analyzer-resolved references across the workspace. Supports Rust, TypeScript/JavaScript, Python, C++, Swift, and unexported, untagged Go fields. Plain reads become getter calls and writes become setter calls. Apply is refused when a same-named access cannot be proven to reference the selected field; `force` does not bypass unresolved references. Validates in-memory analyzer overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the struct or class" },
                    "symbol": { "type": "string", "description": "Field by name (`Class::field` or `field`), or class name when `field` is given" },
                    "field": { "type": "string", "description": "Name of the field to encapsulate" },
                    "class_name": { "type": "string", "description": "Optional name of the class or struct declaring the field" },
                    "line": { "type": "integer", "description": "1-based line of the field's name" },
                    "character": { "type": "integer", "description": "1-based column of the field's name" },
                    "by_value": { "type": "boolean", "description": "Return the field by value (Rust `Copy`, C++ primitive) or by reference" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler checks in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings occur" }
                }
            }),
        },
        McpTool {
            name: "code_migrate_type".to_string(),
            description: "Change a declared type and report the whole shape of what that breaks across Rust, TypeScript/JavaScript, Python, C++, Swift, and Go (Roadmap 7.1.4). Give the declaration's position or symbol name (a struct/class field, a function parameter, a return type, or an annotated variable) and the type it should become; the declaration is rewritten in memory and the workspace is type-checked in one overlay, with every file that references the symbol checked too. With `transitive: true`, downstream variable bindings, matching parameter types, and function return signatures along the data-flow graph are transitively migrated automatically. With `convert: true`, language-idiomatic conversions (`.into()` in Rust, `Number(...)` / `as` in TS, `int(...)` in Python, `int64(...)` in Go, `Int64(...)` in Swift, `static_cast<...>()` in C++) are written at sites where old and new types meet, keeping only conversions accepted by the analyzer. Type-checked in one overlay before anything is written."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the symbol" },
                    "symbol": { "type": "string", "description": "Symbol name to migrate (field, parameter, function return type, variable; alternative to line/character)" },
                    "line": { "type": "integer", "description": "1-based line of the declared name" },
                    "character": { "type": "integer", "description": "1-based column of the declared name" },
                    "to": { "type": "string", "description": "The type it should become, spelled as it will be written" },
                    "convert": { "type": "boolean", "description": "Write language-idiomatic conversions at the sites where old and new types meet, keeping only conversions the analyzer accepts (default false: report only)" },
                    "transitive": { "type": "boolean", "description": "Transitively migrate downstream variable annotations, parameter types, and function return signatures along the data-flow graph (default false)" },
                    "apply": { "type": "boolean", "description": "Write the declaration (default false: report only)" },
                    "force": { "type": "boolean", "description": "Write the declaration while sites still do not fit" }
                },
                "required": ["to"]
            }),
        },
        McpTool {
            name: "code_extract_field".to_string(),
            description: "Promote an expression inside a method into a field of the type the method belongs to across Rust, TypeScript, Python, C++, Swift, and Go. Give the selection (path plus 1-based start and end line/character, range, or expression), the field's name and optional `type`. The method then reads the instance field (`self.<name>`, `this.<name>`, `this-><name>`, or `r.<name>`), the class/struct declares the field, and construction sites or struct literals anywhere in the workspace initialise it with `init` (default: the expression itself); pass `init` for a different starting value, which is required when the expression reads receiver. `replace_all` reads the field at every identical occurrence in the method. The whole change is type-checked in one overlay before anything is written."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File the selection is in" },
                    "line": { "type": "integer", "description": "1-based line where the expression starts" },
                    "character": { "type": "integer", "description": "1-based column where it starts" },
                    "end_line": { "type": "integer", "description": "1-based line where it ends" },
                    "end_character": { "type": "integer", "description": "1-based column where it ends (exclusive)" },
                    "range": { "type": "string", "description": "Selection range as START_LINE:START_COL-END_LINE:END_COL" },
                    "expression": { "type": "string", "description": "Expression text to extract if line/col not given" },
                    "name": { "type": "string", "description": "What the new field is called" },
                    "type": { "type": "string", "description": "The field's type" },
                    "init": { "type": "string", "description": "What every construction site initialises the field with (default: the expression itself)" },
                    "replace_all": { "type": "boolean", "description": "Read the field at every identical occurrence in the method (default false: only the selection)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a pattern would break or the result does not compile" }
                },
                "required": ["path", "name"]
            }),
        },
        McpTool {
            name: "code_generify".to_string(),
            description: "Make a parameter generic across TypeScript, Python, C++, Swift, Go, and Rust (Roadmap 7.1.4): its concrete type becomes a type parameter with the bound it must satisfy. Give the function's name position (or `symbol`), the `param` and optional `bound`; `type_param` names the new parameter (default `T`, refused if the function already has one of that name). Preserves qualifiers, pointers and reference syntax. Updates C++ prototypes in headers. Type-checked in one overlay before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function (optional if symbol is unique)" },
                    "symbol": { "type": "string", "description": "Function or method name to generify (alternative to line and character)" },
                    "line": { "type": "integer", "description": "1-based line of the function's declaration" },
                    "character": { "type": "integer", "description": "1-based column of the function's declaration" },
                    "param": { "type": "string", "description": "The parameter to make generic" },
                    "bound": { "type": "string", "description": "The trait or constraint bound, such as `AsRef<[u32]>`, `Comparable`, `Numeric`, or `any`" },
                    "type_param": { "type": "string", "description": "The new type parameter's name (default `T`)" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the new signature and the check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["param"]
            }),
        },
        McpTool {
            name: "code_invert_boolean".to_string(),
            description: "Invert a predicate: a function returning `bool` gets a new name and the opposite meaning (`is_valid` → `is_invalid`), and every caller keeps doing what it did. Give the function's name position (or `symbol`) and `new_name`. The body returns the negation of what it returned — a one-expression body is negated in place, a longer one as a block, and every `return` of the function (not of a closure or nested `fn` inside it) is negated. Every call becomes `!new_name(…)`, or loses the `!` it had, since the two cancel; a call followed by `.`, `?` or an index is parenthesized. A reference that is not a call — the function used as a value — is named, because it keeps its old meaning under the new name. A recursive predicate is refused. At a `bool` field or a `let` binding instead, the value is inverted: every read gains a `!` or loses the one it had (parenthesized when it goes on), and every write stores the negation — an assignment, the `let` initialiser, the field in a struct literal or its shorthand. A borrow, a compound assignment (`|=`), a pattern that binds it, a use in a format string, a derived `Default` or a serde derive cannot keep their meaning and block the write unless `force`; a local without a `: bool` annotation is inverted only when the analyzer says it is `bool`. Type-checked in one overlay; `verify: \"compile\"` adds compiler checks. Rust, TypeScript, JavaScript, Python, C++, Swift, Go."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function, field or variable" },
                    "line": { "type": "integer", "description": "1-based line of its name" },
                    "character": { "type": "integer", "description": "1-based column of its name" },
                    "symbol": { "type": "string", "description": "Predicate name or symbol (`isValid`, `Math::isValid`)" },
                    "function": { "type": "string", "description": "Predicate function name" },
                    "new_name": { "type": "string", "description": "The name of the inverted predicate, field or variable" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["new_name"]
            }),
        },
        McpTool {
            name: "code_make_static".to_string(),
            description: "Turn a method that never accesses instance state into a static / associated function, with every call site. Give the method's name position, or `symbol` (`Type::method`, `Type.method`), or `path` with `method` and optional `class_name`. In Rust, the receiver (`&self`, `&mut self`, `self`) leaves the declaration; `value.method(args)` becomes `Type::method(args)`. In TypeScript/JavaScript, `static` is added; in Python, `@staticmethod` is added and `self` removed; in C++, `static` is added and trailing `const` removed; in Swift, `func` becomes `static func`; in Go, the receiver clause is removed. A receiver that does something when it is evaluated cannot be dropped silently without `force`. Rust, TypeScript, JavaScript, Python, C++, Swift, Go."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the method" },
                    "symbol": { "type": "string", "description": "The method by name (`Type::method`, `Type.method`), or a file with line" },
                    "method": { "type": "string", "description": "Name of the method to make static" },
                    "class_name": { "type": "string", "description": "Optional class or struct name declaring the method" },
                    "line": { "type": "integer", "description": "1-based line of the method's name" },
                    "character": { "type": "integer", "description": "1-based column of the method's name" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a receiver with effects would be dropped or the result does not compile" }
                }
            }),
        },
        McpTool {
            name: "code_convert_to_method".to_string(),
            description: "Turn a static or associated function into an instance method, with every call site: the inverse of `code_make_static`. Give the function's name position, or `symbol` (`Type::method`, `Type.method`), or `path` with `method` and optional `class_name`. Its first parameter (of the target type) becomes the receiver (`self`, `this`, `*this`, or Go receiver). In TypeScript/JavaScript/C++/Swift, `static` is removed and the first parameter is dropped. In Python, `@staticmethod` is removed and first parameter becomes `self`. In Go, the free function gets a receiver `(r *Type)`. Call sites rewrite from `Type.method(inst, args)` to `inst.method(args)`. Rust, TypeScript, JavaScript, Python, C++, Swift, Go."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function" },
                    "symbol": { "type": "string", "description": "The function by name (`Type::function`, `Type.function`), or a file with line" },
                    "method": { "type": "string", "description": "Name of the function to convert to method" },
                    "class_name": { "type": "string", "description": "Optional class or struct name declaring the function" },
                    "line": { "type": "integer", "description": "1-based line of the function's name" },
                    "character": { "type": "integer", "description": "1-based column of the function's name" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                }
            }),
        },
        McpTool {
            name: "code_inline_parameter".to_string(),
            description: "Inline a parameter that every caller passes the same constant for: the value is bound at the top of the body (`let max: u32 = LIMIT;`), and the parameter leaves the declaration and its argument every call. Give the parameter's position, or `symbol` / `function` with `parameter`. The value must mean the same in the body as at the call — a literal, a constant, or a path (`Mode::Fast`, `Config.MAX`); a lowercase name may be a local of the caller and is refused, and so are calls that pass different values (each is listed). The function used as a value, or a call inside the function itself, blocks the write unless `force`. Type-checked in one overlay before anything is written. Rust, TypeScript, JavaScript, Python, C++, Swift, Go."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function" },
                    "line": { "type": "integer", "description": "1-based line of the parameter's name" },
                    "character": { "type": "integer", "description": "1-based column of the parameter's name" },
                    "symbol": { "type": "string", "description": "Function name or symbol (`clamp`, `Math::clamp`, `Math.clamp`)" },
                    "function": { "type": "string", "description": "Function name" },
                    "parameter": { "type": "string", "description": "Name of parameter to inline" },
                    "param": { "type": "string", "description": "Alternative alias for parameter" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile; a reference that is not a call still blocks the write" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_extract_delegate".to_string(),
            description: "Extract a delegate (Extract Class): the fields you name leave a struct or class for a new helper type the struct/class then holds, with the methods you name that use only those fields. The owner keeps a forwarding method of the same signature for each moved method, so callers do not change; external access to moved fields goes through the new field (`a.city` becomes `a.address.city`), and struct literals build the helper. Supported across TypeScript/JavaScript, Python, C++, Swift, Go, and Rust. Type-checked in one overlay before anything is written."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the struct or class" },
                    "line": { "type": "integer", "description": "1-based line of the `struct`/`class` keyword (optional if `symbol` is given)" },
                    "character": { "type": "integer", "description": "1-based column on that line (optional if `symbol` is given)" },
                    "symbol": { "type": "string", "description": "Name of the struct or class (alternative to line/character)" },
                    "fields": { "type": "array", "items": { "type": "string" }, "description": "Fields that move into the helper" },
                    "methods": { "type": "array", "items": { "type": "string" }, "description": "Methods that move with them (may use only those fields)" },
                    "name": { "type": "string", "description": "Name of the helper type" },
                    "field": { "type": "string", "description": "Name of the field that holds the helper" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "fields", "name", "field"]
            }),
        },
        McpTool {
            name: "code_extract_trait".to_string(),
            description: "Extract a trait from the methods you name of an inherent `impl Type` block (rust-analyzer's `generate_trait_from_impl` takes every method, keeps the trait private and leaves callers in other modules without it in scope). The named methods move into `trait Name` and `impl Name for Type`; the rest stay inherent (the block goes when it empties). Doc comments go to the trait's declarations, attributes stay on the implementation, and the trait is as visible as the widest moved method. Every other file that references a moved method gets `use …::Name;`. Type-checked in one overlay before anything is written. Caller type annotations are migrated across the workspace where safe unless `migrate_callers: false`. Generic impl blocks and trait implementations are refused. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the `impl` block" },
                    "line": { "type": "integer", "description": "1-based line of the `impl` header (or any line inside the block)" },
                    "character": { "type": "integer", "description": "1-based column on that line" },
                    "methods": { "type": "array", "items": { "type": "string" }, "description": "Names of the methods that move into the trait" },
                    "name": { "type": "string", "description": "Name of the new trait" },
                    "migrate_callers": { "type": "boolean", "description": "Whether to migrate caller parameter and variable type annotations from concrete type to extracted trait when caller only uses extracted methods (default true)" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "methods", "name"]
            }),
        },
        McpTool {
            name: "code_loop_to_iterator".to_string(),
            description: "Turn an accumulating `for` loop into an iterator chain, stream expression, comprehension, or functional pipeline across TypeScript/JavaScript, Python, Swift, Go, C++, and Rust (Roadmap 7.1.5). Recognised patterns: sum/reduction, count of matching elements, collection/mapping (`push`, `append`, etc.), find/search (`find`, `find_map`), and boolean predicates (`any`, `all`). Addressable by `symbol` (function or accumulator name) or `line`/`character`. Type-checked before anything is written."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the loop" },
                    "symbol": { "type": "string", "description": "Optional function or accumulator name identifying the loop" },
                    "line": { "type": "integer", "description": "1-based line of the `for` loop" },
                    "character": { "type": "integer", "description": "1-based column on that line" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_replace_constructor_with_factory".to_string(),
            description: "Replace raw struct/class instantiations with a named static factory method across the codebase (Roadmap 7.1.3). Generates the factory method declaration (`pub fn new(...) -> Self` in Rust, `func New<Type>(...) *<Type>` in Go, `static create(...)` in TS/JS/Python/C++/Swift) and rewrites raw instantiations (`Type { field1, field2 }`, `&Type{...}`, `new Type(...)`, `Type(...)`) into calls to the factory method, preserving argument evaluation order. Supports Rust, Go, TypeScript, JavaScript, Python, C++, and Swift. Type-checked via analyzer overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the struct or class" },
                    "type_name": { "type": "string", "description": "Name of the struct or class" },
                    "factory_name": { "type": "string", "description": "Name of the factory method (default: language convention, e.g. `new`, `New<Type>`, `create`)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                },
                "required": ["path", "type_name"]
            }),
        },
        McpTool {
            name: "code_replace_constructor_with_builder".to_string(),
            description: "Replace raw struct/class instantiations with a fluent builder pattern across the codebase (Roadmap 7.1.3). Generates a builder type (`<Type>Builder`) with fluent setter methods and a `build()` method, generates `<Type>::builder()` / `New<Type>Builder()`, and rewrites raw instantiations (`Type { field1: val1, ... }`) into builder chains (`Type::builder().field1(val1)...build()`). Supports Rust, Go, TypeScript, JavaScript, Python, C++, and Swift. Type-checked via analyzer overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the struct or class" },
                    "type_name": { "type": "string", "description": "Name of the struct or class" },
                    "builder_name": { "type": "string", "description": "Name of the builder type (default: `<Type>Builder`)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                },
                "required": ["path", "type_name"]
            }),
        },
        McpTool {
            name: "code_pull_up".to_string(),
            description: "Pull up members (methods, fields, properties, constants) from a subclass or sub-trait into its superclass or super-trait across Python, TypeScript/JavaScript, C++, Swift, and Rust trait hierarchies (Roadmap 7.1.3). Cleans up redundant overrides in sibling subclasses, adjusts indentation and modifiers (such as stripping `override`), detects collisions, and validates overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the subclass or sub-trait" },
                    "class_name": { "type": "string", "description": "Name of the subclass, derived class, or sub-trait" },
                    "symbol": { "type": "string", "description": "Alternative alias for class_name" },
                    "members": { "type": "array", "items": { "type": "string" }, "description": "Names of members to pull up" },
                    "target_class": { "type": "string", "description": "Optional name of the superclass (auto-detected from inheritance if omitted)" },
                    "clean_siblings": { "type": "boolean", "description": "Whether to also remove identical duplicate members from sibling subclasses (default true)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler checks on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                },
                "required": ["members"]
            }),
        },
        McpTool {
            name: "code_push_down".to_string(),
            description: "Push down members (methods, fields, properties, constants) from a superclass or super-trait into specific or all direct subclasses or sub-traits across Python, TypeScript/JavaScript, C++, Swift, and Rust trait hierarchies (Roadmap 7.1.3). Adjusts indentation and modifiers, detects collisions, and validates overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the superclass or super-trait" },
                    "class_name": { "type": "string", "description": "Name of the superclass, base class, or super-trait" },
                    "symbol": { "type": "string", "description": "Alternative alias for class_name" },
                    "members": { "type": "array", "items": { "type": "string" }, "description": "Names of members to push down" },
                    "target_classes": { "type": "array", "items": { "type": "string" }, "description": "Optional list of specific subclass names to push down to (all discovered subclasses if omitted)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler checks on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                },
                "required": ["members"]
            }),
        },
        McpTool {
            name: "code_replace_inheritance_with_delegation".to_string(),
            description: "Replace inheritance with composition and delegation across Python, TypeScript/JavaScript, C++, and Swift (Roadmap 7.1.3). Decouples a subclass from its base class, introduces an encapsulated private delegate field, initializes it in constructors/initializers, auto-generates forwarding methods to maintain API compatibility, and strips invalid `override` modifiers and `super` calls. Type-checked via analyzer overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the subclass" },
                    "sub_type": { "type": "string", "description": "Name of the subclass to refactor" },
                    "symbol": { "type": "string", "description": "Alternative alias for sub_type" },
                    "base_type": { "type": "string", "description": "Optional name of the base class to decouple from (auto-detected if omitted)" },
                    "field_name": { "type": "string", "description": "Optional name for the delegate field (defaults to base class name in snake_case/camelCase)" },
                    "methods": { "type": "array", "items": { "type": "string" }, "description": "Optional explicit list of method names to forward (auto-discovered if omitted)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler checks on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                }
            }),
        },
        McpTool {
            name: "code_replace_conditional_with_polymorphism".to_string(),
            description: "Replace conditional logic (switch/match statements or if-elif-else cascades) with polymorphic dispatch across Python, TypeScript/JavaScript, C++, Swift, and Rust (Roadmap 7.1.5). Generates base class, interface, protocol, or trait hierarchy with polymorphic method and replaces conditional block with dynamic dispatch. Type-checked via analyzer overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the conditional statement" },
                    "line": { "type": "integer", "description": "1-based line of the switch/match/if statement" },
                    "character": { "type": "integer", "description": "1-based column of the switch/match/if statement" },
                    "symbol": { "type": "string", "description": "Optional function or method name containing the conditional" },
                    "base_name": { "type": "string", "description": "Name of the base class, interface, protocol, or trait (e.g. `Bird`, `Employee`, `Shape`)" },
                    "method_name": { "type": "string", "description": "Name of the polymorphic method to generate (e.g. `get_speed`, `calculate_pay`, `area`)" },
                    "params": { "type": "array", "items": { "type": "string" }, "description": "Optional method parameter definitions (e.g. `[\"amount: number\"]`)" },
                    "return_type": { "type": "string", "description": "Optional return type of the method (e.g. `number`, `float`, `f64`)" },
                    "target_var": { "type": "string", "description": "Optional target variable to invoke method on (defaults to discriminator expression)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler checks on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                },
                "required": ["base_name", "method_name"]
            }),
        },
        McpTool {
            name: "code_extract_interface".to_string(),
            description: "Extract an interface, protocol, or abstract class from a class or struct across TypeScript/JavaScript, Go, Python, C++, Swift, and Rust (Roadmap 7.1). Extracts selected public method contracts, generates the interface definition, updates the class/struct to implement or conform to it, and migrates caller type annotations across the workspace where safe unless migrate_callers: false. Type-checked via analyzer overlays before writing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the class or struct" },
                    "symbol": { "type": "string", "description": "Name of the class, struct, or type to extract an interface from" },
                    "interface_name": { "type": "string", "description": "Name of the new interface, protocol, or abstract class" },
                    "methods": { "type": "array", "items": { "type": "string" }, "description": "Optional subset of method names to include in the interface (defaults to all public methods)" },
                    "line": { "type": "integer", "description": "Optional 1-based line of the type declaration" },
                    "character": { "type": "integer", "description": "Optional 1-based column of the type declaration" },
                    "migrate_callers": { "type": "boolean", "description": "Update caller functions/methods across the workspace that accept the concrete class/struct to accept the new interface/trait when safe (default true)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler checks on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when analyzer warnings or non-fatal diagnostics occur" }
                },
                "required": ["symbol", "interface_name"]
            }),
        },
        McpTool {
            name: "code_extract_function".to_string(),
            description: "Extract the selected code into a new function with the name you give across TypeScript/JavaScript, Python, Go, C++, Swift, and Rust (Roadmap 7.1.2), and replace duplicates with the same call. Structural duplicates within the file and across the workspace (`other_files: true`) are detected and replaced; parameterizing differing literals is supported with `parameterize: true`. For Rust, rust-analyzer's `extract_function` performs the initial extraction, and borrow safety is verified with `compile_gate`. Set `duplicates: false` to extract the selection alone."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the selection" },
                    "line": { "type": "integer", "description": "1-based line where the selection starts" },
                    "character": { "type": "integer", "description": "1-based column where the selection starts" },
                    "end_line": { "type": "integer", "description": "1-based line where the selection ends" },
                    "end_character": { "type": "integer", "description": "1-based column just past the selection" },
                    "name": { "type": "string", "description": "Name of the new function" },
                    "duplicates": { "type": "boolean", "description": "Also replace the other places in the file with the same code (default true)" },
                    "parameterize": { "type": "boolean", "description": "Also take places that differ from the selection only in literals (a number, a string, a char at the same position): each literal that differs becomes a parameter of the new function, typed as the analyzer types the selection's own, and every call passes its place's literal (default false)" },
                    "other_files": { "type": "boolean", "description": "Also look in the crate's other files: a copy there calls the function through its module path, and the function becomes `pub(crate)` when that is needed (default false)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it (always done when a duplicate is replaced)" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "end_line", "end_character", "name"]
            }),
        },
        McpTool {
            name: "code_introduce_variable".to_string(),
            description: "Introduce a variable for an expression and replace every occurrence of it in the enclosing function, not only the selected one: `(w + 1)` three times becomes `let w1 = w + 1;` above the first and `w1` at each place. Give the selection (`line`/`character` to `end_line`/`end_character`) and `name`. Refused when evaluating once is not the same as evaluating at each place: the expression calls something, expands a macro, uses `?` or awaits, or a name it reads is assigned, mutably borrowed or rebound between the first occurrence and the last (or in a loop that runs a later one again). For one occurrence of such an expression use `code_assist` with `extract_variable`. Type-checked in one overlay before anything is written. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the expression" },
                    "line": { "type": "integer", "description": "1-based line where the selection starts" },
                    "character": { "type": "integer", "description": "1-based column where the selection starts" },
                    "end_line": { "type": "integer", "description": "1-based line where the selection ends" },
                    "end_character": { "type": "integer", "description": "1-based column just past the selection" },
                    "name": { "type": "string", "description": "Name of the new variable" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "end_line", "end_character", "name"]
            }),
        },
        McpTool {
            name: "code_wrap_return".to_string(),
            description: "Wrap what a function returns across TypeScript/JavaScript, Python, C++, Swift, Go, and Rust (Roadmap 7.1.4). Supports `promise` (adds `async`, rewrites callers to `await`), `option`/`nullable` (`Optional`, `std::optional`, `T?`, `T | null`), `result`/`expected` (`Result`, `std::expected`, `(T, error)`), `pointer` (`*T`), and custom envelope types (e.g. `Response`, `CustomEnvelope`). Optional `constructor` specifies a custom factory or constructor expression (e.g. `Response::ok`, `new Response`, `Response(val)`). Callers that cannot propagate the wrapped type are reported as blocked and require a decision (or `force`). Addressable by `symbol` or `line`/`character`."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function (optional if symbol is unique)" },
                    "symbol": { "type": "string", "description": "Function or method name to wrap (alternative to line and character)" },
                    "line": { "type": "integer", "description": "1-based line of the function's declaration" },
                    "character": { "type": "integer", "description": "1-based column of the function's declaration" },
                    "wrapper": { "type": "string", "description": "What the return type becomes wrapped in: `promise`, `option`/`nullable`, `result`/`expected`/`error`, `pointer`, or a custom envelope type name" },
                    "constructor": { "type": "string", "description": "Optional constructor or factory expression for wrapping return expressions (e.g. `Response::ok`, `new Response`, `Response(val)`)" },
                    "error": { "type": "string", "description": "For `result`: the error type, such as `anyhow::Error`, `Error`, or `std::string`" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a caller cannot propagate or the result does not compile" }
                },
                "required": ["wrapper"]
            }),
        },
        McpTool {
            name: "code_extract_parameter".to_string(),
            description: "Promote an expression inside a function into a parameter of it, passing what the body used to say at every existing call site — so no caller changes behaviour and the next one can choose. Give the selection (path plus 1-based start and end line/character) and the parameter's name; the type comes from the literal or the analyzer when it gives one in a shape this can read, otherwise pass `type`. Works in Rust, TypeScript, JavaScript, Python, Go, C, C++ and Swift, on functions and methods, with each language's spelling: `name: T` in Rust and TypeScript, `name T` in Go, `name` or `name: T` in Python (untyped when no type is known), `name` in JavaScript, which takes no type, `T name` in C and C++ (`80` is `int`, `0.5` is `double`, a string literal `const char *`), and in Swift `name: T` with callers passing `name: value` when every existing parameter is labeled, otherwise `_ name: T` with the value passed bare. A C or C++ function declared apart from its definition (in a header, or a method declared in its class) gets the parameter in that declaration too. The parameter is added at the end of the list, keeping the list's shape, and the argument at the end of every call; a list that ends in a rest or variadic parameter (`...rest`, `...T`, `*args`, C's `...`, an unlabeled Swift variadic) is refused, because the new argument would not reach the new parameter. `replace_all` puts the parameter in every identical occurrence inside the body rather than only the selected one. A reference that is not a call with this arity is named rather than mangled; imports are left alone. The whole change is type-checked in one overlay before anything is written: an expression that names a local or anything private to the function it came from cannot be spelled at a call site, and that is what the check reports."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File the selection is in" },
                    "line": { "type": "integer", "description": "1-based line where the expression starts" },
                    "character": { "type": "integer", "description": "1-based column where it starts" },
                    "end_line": { "type": "integer", "description": "1-based line where it ends" },
                    "end_character": { "type": "integer", "description": "1-based column where it ends (exclusive)" },
                    "name": { "type": "string", "description": "What the new parameter is called" },
                    "type": { "type": "string", "description": "The parameter's type, when the analyzer gives none" },
                    "replace_all": { "type": "boolean", "description": "Replace every identical occurrence in the body (default false: only the selection)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile` (Rust files only): also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "end_line", "end_character", "name"]
            }),
        },
        McpTool {
            name: "code_introduce_parameter_object".to_string(),
            description: "Bundle two or more named parameters into one object and rewrite their body uses and call sites. Supports Rust, TypeScript, JavaScript, Python, Go, C, C++ and Swift. Rust generates a struct; TypeScript an interface; JavaScript passes a plain object without a type declaration; Python a dataclass or an unannotated class; Go, C/C++ and Swift a struct. C/C++ prototypes and definitions are updated together, including headers; C++ literal syntax follows the configured language version. Python keyword arguments and Swift labels are matched by name. Literal fields preserve the call's argument order; bundling nonadjacent arguments is refused when the planner cannot show that reordered expressions commute. Failed reference queries and unreadable referenced files stop the refactor. References it does not rewrite (the function used as a value, a position where the file says something else) are reported, and nothing is written while one remains, with or without force or verify. JavaScript refuses spread/apply calls, arguments-dependent bodies, destructured parameters, unsupported defaults and stale references; its validation uses syntax diagnostics rather than a type-check guarantee. Other languages use analyzer diagnostics on the proposed texts; Rust additionally supports verify=compile. Inspect the reported limitations and diff before apply=true; force only bypasses the diagnostic gate, not structural refusals. Re-run your formatter afterwards."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the declaration" },
                    "character": { "type": "integer", "description": "1-based column of the declaration" },
                    "params": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The parameters to bundle, by name; two or more. Order does not matter — the struct keeps the declaration's order."
                    },
                    "name": { "type": "string", "description": "The new type's name, UpperCamelCase (`Opts`, `SyncRequest`); JavaScript uses it to derive the binding only; a Go type may be lower case, which leaves it unexported, and so may a C or C++ struct" },
                    "binding": { "type": "string", "description": "What the new parameter is called in the body (default: the type's name in snake_case in Rust, Python, C and C++, lowerCamelCase in TypeScript, JavaScript, Go and Swift)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Rust only. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and analyzer diagnostics only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["params", "name"]
            }),
        },
        McpTool {
            name: "code_move".to_string(),
            description: "Move a declaration (function, class, struct, enum, interface, trait, const) into another module across Rust, TypeScript, JavaScript, Python, Go, C/C++, and Swift, with the imports that keep every user of it compiling. The item travels whole — signature, body, doc comment, attributes — is cut from its file and appended to the target; every file using it has its imports updated or path-qualified references requalified. A target file that does not exist yet is created (and declared in its parent module for Rust or initialized with package/header for Go/C++), in the same change. The whole change is type-checked in one overlay first, so a move that would not compile is reported rather than written. Nothing is written without `apply`."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the item (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the declaration" },
                    "character": { "type": "integer", "description": "1-based column of the declaration" },
                    "to": { "type": "string", "description": "The target module's file, e.g. `crates/x/src/fixture.rs`" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the move (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["to"]
            }),
        },
        McpTool {
            name: "code_move_module".to_string(),
            description: "Move a whole module to another parent: `a::b` becomes `c::b`. Give the module's file (`path`, e.g. `src/a/b.rs` or `src/a/b/mod.rs`) and where its file goes (`to`, e.g. `src/c/b.rs`); the name stays. The file moves with the directory of its submodules; the `mod b;` declaration (with its attributes and doc comment) leaves the old parent and is declared in the new one with the same visibility; every path the analyzer lists as naming the module is spelled anew (a qualified path gets the new parent, a bare use in the old parent gets an import, a grouped import is narrowed and the module imported on its own line, an import in the new parent that would clash with the declaration is dropped); `super::` in the moved file becomes the old parent's absolute path. The whole change is type-checked in one overlay first, so something private to the old parent that the module used is reported, not written. `verify: \"compile\"` adds `cargo check`. Nothing is written without `apply`. Rust, ordinary crate layout only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The module's file, e.g. `src/a/b.rs` or `src/a/b/mod.rs`" },
                    "to": { "type": "string", "description": "Where the module's file goes, e.g. `src/c/b.rs`; its parent module must exist" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the move (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "to"]
            }),
        },
        McpTool {
            name: "code_move_method".to_string(),
            description: "Move a method to the type of one of its parameters: `Order::price_with(&self, tax: &Tax)` becomes `Tax::price_with(&self, order: &Order)`. Give the method's name position and `to_param`. The parameter becomes the receiver, borrowed as it was (`&Tax` -> `&self`); the old receiver becomes a parameter in its place, typed as it was borrowed; in the body `self` becomes that parameter, the parameter becomes `self`, and `Self` is spelled out. The method goes into the new type's inherent `impl` (one is made after the type when there is none), and every call swaps the two: `o.price_with(t, 1)` -> `t.price_with(&o, 1)`, `Order::price_with(o, t, 1)` -> `Tax::price_with(t, o, 1)`. A call whose receiver or argument does something (the order they run in would change), a method used as a value, a recursive method, a trait implementation and a generic `impl` are refused. Type-checked in one overlay first; nothing is written without `apply`. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the method" },
                    "line": { "type": "integer", "description": "1-based line of the method's name" },
                    "character": { "type": "integer", "description": "1-based column of the method's name" },
                    "to_param": { "type": "string", "description": "The parameter whose type the method moves to" },
                    "to_type": { "type": "string", "description": "For an associated function (no `self`): the struct or enum it moves to; every `Old::f` path becomes `New::f`" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a call's evaluation order changes or the result does not compile; a reference that is not rewritten still blocks the write" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_change_signature".to_string(),
            description: "Preferred tool for changing function signatures: Use this tool when changing function parameters, return type, visibility, or asyncness instead of manual multi-file edits. Change what a function takes, with its call sites, across TypeScript/JavaScript, Python, C++, Swift, Go, and Rust (Roadmap 7.1.1). `params` is the parameter list the function should end up with: `name` keeps the parameter declared under that name in this position, `name: Type = expression` adds one and passes `expression` at every call site, and a declared parameter that is not listed is removed. Synchronizes header prototypes in C++, preserves `self`/`this` receivers in methods, and handles keyword arguments and Swift parameter labels. Dropping a parameter the body still uses is refused with the usages unless `force`. `returns` modifies the return type, `visibility` modifies visibility, and `async` makes the function async and awaits every call. The whole change is type-checked together via analyzer overlays before it is written. Go (a `.go` file, through gopls v0.23.0) reorders named parameters and removes provably unused ones. List each kept parameter once in its new order, or pass an empty array to remove all. Both body inspection and gopls references must prove every removed parameter unused. Dropped arguments must be literals or simple variables: calls, selectors, indexing, receives, conversions and operators are refused because they may have effects or panic. Grouped parameters move one by one; receivers and results stay as declared for parameter changes. A retained variadic parameter stays last; removing it removes the whole argument tail, including spread arguments only when safely removable. Every declaration and call must match the requested list, with comments preserved and no unrelated edits, and the whole proposal is type-checked before writing. Force overrides none of these refusals. Go also adds explicitly typed primitive parameters with numeric, string or rune literal arguments to ordinary non-generic functions and named value/pointer receiver methods, retaining every old parameter in its original order and preserving receiver evaluation. These additions require remote compiler verification of packages and test callers before preview or apply, with the node's build flags preserved. With `returns`, Go also replaces one existing unnamed primitive result of an ordinary non-generic, non-variadic free function while every named parameter remains exactly unchanged. Both old and new result names must be unshadowed predeclared primitive types; complete direct-call evidence and remote compiler verification of packages and test callers are required even for a no-op preview. Method values/expressions, interface obligations or dispatch, generic receivers, receiver-name capture, variadics, grouped-parameter interior insertion, scope-dependent defaults, combined additions with reorders/removals, parameter type changes, named/multiple/void/composite results, receiver or generic result changes, parameter changes combined with `returns`, scope-dependent result types, visibility changes, async, verify, unnamed or blank parameters, generic removals, generic functions with calls, function values and unreconciled calls remain unsupported. Uncertain argument reordering is refused; `true`, `false` and `nil` count as variables because a Go scope may redeclare them."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the declaration" },
                    "character": { "type": "integer", "description": "1-based column of the declaration" },
                    "params": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The whole new parameter list, in order: `name` to keep, `name: Type = expression` to add; omit one to remove it. Receivers (`&self`, `self`, `this`) are never listed. Go: every kept parameter by name, once, in the new order; omitted ones must be provably unused with safely removable arguments. An empty array requests removal of all parameters."
                    },
                    "returns": { "type": "string", "description": "New return type for the function. Supported across Rust, TypeScript, Python, C++, Swift, and Go" },
                    "visibility": { "type": "string", "description": "New visibility modifier (`export`, `public`, `private`, etc.)" },
                    "async": { "type": "boolean", "description": "`true` makes the function `async` and appends `await` to calls; `false` removes both. Supported across Rust, TypeScript, Python, and Swift" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run compiler check on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Refused for a Go file before anything is planned or written" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Drop a parameter the body still uses, and write even when the result does not compile. Overrides no Go refusal" }
                },
                "required": ["params"]
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
    ];
    for tool in &mut tools {
        if let Some(properties) = tool
            .input_schema
            .get_mut("properties")
            .and_then(|v| v.as_object_mut())
        {
            for key in POSITION_ARGUMENTS {
                if let Some(property) = properties.get_mut(key) {
                    property["minimum"] = serde_json::json!(1);
                    property["maximum"] = serde_json::json!(u32::MAX);
                }
            }
            for key in ["end_line", "end_character"] {
                if let Some(description) = properties
                    .get_mut(key)
                    .and_then(|p| p.get_mut("description"))
                    && let Some(text) = description.as_str()
                {
                    *description = serde_json::json!(format!(
                        "{text}; supply both end_line and end_character, with the end at or after the start"
                    ));
                }
            }
        }
        if SYMBOL_ADDRESSABLE.contains(&tool.name.as_str()) {
            relax_position_schema(&mut tool.input_schema);
        }
    }
    tools
}

/// Symbol-addressable tools accept `symbol` instead of a position: advertise the property and
/// stop requiring path/line/character, otherwise a schema-validating client cannot use the
/// name-based form at all.
pub fn relax_position_schema(schema: &mut serde_json::Value) {
    if let Some(props) = schema.get_mut("properties").and_then(|p| p.as_object_mut()) {
        props.entry("symbol").or_insert_with(|| {
            serde_json::json!({
                "type": "string",
                "description": "Symbol name instead of path/line/character, optionally qualified (`Metrics::record`, `pkg.Func`, `Class.method`); resolved through the workspace symbol index. `path` may still be given to disambiguate."
            })
        });
    }
    if let Some(required) = schema.get_mut("required").and_then(|r| r.as_array_mut()) {
        required.retain(|r| !matches!(r.as_str(), Some("path") | Some("line") | Some("character")));
    }
}
