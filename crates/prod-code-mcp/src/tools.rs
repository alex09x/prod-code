use crate::protocol::{McpTool, McpToolCallResult};
use crate::sync::scan_workspace_files;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, SyncRequest, WireMessage};
use std::net::SocketAddr;
use std::path::Path;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use url::Url;

/// Return list of tools exposed by the MCP server.
pub fn list_tools() -> Vec<McpTool> {
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
            description: "Lint the whole workspace on the remote gateway (cargo clippy -D warnings / go vet) and return structured findings with file:line:col. `fix: true` (Rust) applies every fix clippy and rustc mark machine-applicable, then lints again."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "env": { "type": "object", "additionalProperties": { "type": "string" }, "description": "Environment variables for the command, e.g. {\"RUST_BACKTRACE\": \"1\"}" },
                    "timeout_secs": { "type": "integer", "description": "Kill after this many seconds (default 3600)" },
                    "fix": { "type": "boolean", "description": "Apply the fixes, then lint again (default false): Rust takes clippy's machine-applicable suggestions; Python, TypeScript and C++ run the linter's own fix mode (`ruff check --fix`, `eslint --fix` / `biome lint --write`, `clang-tidy -fix`) on the node and bring the rewritten files back; Go's `go vet` has none" },
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
            description: "What a type implements, or what a trait requires: the upward half of the type hierarchy (`code_implementations` is the downward half). For a Rust type, the traits it implements, derived or written as impl blocks, each with the position of the impl or the derive; inherent impls are not listed. For a Rust trait, its supertraits. Other languages ask their server's own type hierarchy (clangd and gopls answer it) and say so when it has none. Give `symbol` or a file position."
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
            name: "code_safe_delete".to_string(),
            description: "Delete the item (function, type, const, field, module) at a 1-based position only if nothing in the workspace references it; otherwise returns the list of usages that block the deletion. At a parameter of a function, the parameter is removed together with its argument at every call site (the same rewrite as `code_change_signature`), refused while the body still uses it, and type-checked before it is written. The edit is written into the checkout."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the item's name" },
                    "character": { "type": "integer", "description": "1-based column of the item's name" },
                    "force": { "type": "boolean", "description": "For a parameter: remove it even when the body uses it or the result does not compile" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_rename".to_string(),
            description: "Semantic rename of a symbol (type, function, field, variable, module) across the whole workspace, named with `symbol` or located by path + 1-based line/column, driven by the remote analyzer. The result is type-checked in one overlay before anything is written, and a rename that does not compile — typically a new name already declared in the same scope — is refused with the errors unless `force` is given. Rewrites every affected file in the checkout (and renames module files) and reports what changed."
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
                    "force": { "type": "boolean", "description": "Allow a short name, a large number of occurrences, and writing a result that does not compile" }
                },
                "required": ["field", "to"]
            }),
        },
        McpTool {
            name: "code_encapsulate_field".to_string(),
            description: "Make a public field private and turn every access to it outside its declaring file into a method call: reads become `x.field()`, plain writes become `x.set_field(v)`. Give the field's position in its struct (or `symbol`). The getter returns the value for a primitive `Copy` type, a shared reference otherwise (override with `by_value`); the setter is generated only when something writes the field. Both go into the struct's first inherent `impl` in that file, or a new one after the struct, with the field's old visibility. Accesses inside the declaring file stay direct, because a private field is still visible there. A use that cannot become a method call — a struct literal or pattern outside the file, a compound assignment, `&mut x.field` — is reported with its source line, and nothing is written while one remains. The whole change is type-checked in one overlay before anything is written; the analyzer does not check borrows, so where a read goes on to call a method on the field, ask for `verify: \"compile\"`. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the struct" },
                    "line": { "type": "integer", "description": "1-based line of the field's name" },
                    "character": { "type": "integer", "description": "1-based column of the field's name" },
                    "by_value": { "type": "boolean", "description": "Return the field by value (it must be `Copy`) or by shared reference; default: by value for primitive `Copy` types only" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds), and it is the only check that sees a borrow the getter no longer allows" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a use cannot be rewritten or the result does not compile" }
                }
            }),
        },
        McpTool {
            name: "code_migrate_type".to_string(),
            description: "Change a declared type and report the whole shape of what that breaks, before any of it is done. Give the declaration's position (a struct field, a function parameter, a return type, or an annotated `let`) and the type it should become; the declaration is rewritten in memory and the workspace is type-checked in one overlay, with every file that references the symbol checked too. The errors that come back are not a failure, they are the work list: each is reported with its file, line and the source at that line, grouped by file. Where an error is exactly the old type meeting the new one, the report says what conversion would fix that site. With `convert: true` it writes `.into()` at every site where the old and new types meet and checks the overlay again: a conversion is kept only where the analyzer accepts it, a rejected one is taken back and its site stays in the report marked as tried, and if the kept conversions cause an error anywhere else none is kept. Narrowings (`u64` to `u32`) and fallible conversions therefore stay a person's decision. `apply` writes the declaration alone and refuses while any site remains, so a half-migrated type is never written by accident. `apply` then writes the declaration with the kept conversions. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the symbol" },
                    "line": { "type": "integer", "description": "1-based line of the declared name" },
                    "character": { "type": "integer", "description": "1-based column of the declared name" },
                    "to": { "type": "string", "description": "The type it should become, spelled as it will be written" },
                    "convert": { "type": "boolean", "description": "Write `.into()` at the sites where the old and new types meet, keeping only the conversions the analyzer accepts (default false: report only)" },
                    "apply": { "type": "boolean", "description": "Write the declaration (default false: report only)" },
                    "force": { "type": "boolean", "description": "Write the declaration while sites still do not fit" }
                },
                "required": ["to"]
            }),
        },
        McpTool {
            name: "code_extract_field".to_string(),
            description: "Promote an expression inside a method into a field of the type the method belongs to. Give the selection (path plus 1-based start and end line/character), the field's name and its `type`. The method then reads `self.<name>`, the struct declares the field last, and every place that builds the struct — `Type { … }` and `Self { … }` anywhere in the workspace — initialises it, by default with the expression itself; pass `init` for a different starting value, which is required when the expression reads `self`. A pattern that lists every field no longer matches a struct with one more and is reported, not rewritten; nothing is written while one remains. `replace_all` reads the field at every identical occurrence in the method. The whole change is type-checked in one overlay before anything is written; the analyzer does not check borrows, so for a type that is not `Copy`, ask for `verify: \"compile\"`. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File the selection is in" },
                    "line": { "type": "integer", "description": "1-based line where the expression starts" },
                    "character": { "type": "integer", "description": "1-based column where it starts" },
                    "end_line": { "type": "integer", "description": "1-based line where it ends" },
                    "end_character": { "type": "integer", "description": "1-based column where it ends (exclusive)" },
                    "name": { "type": "string", "description": "What the new field is called" },
                    "type": { "type": "string", "description": "The field's type" },
                    "init": { "type": "string", "description": "What every construction site initialises the field with (default: the expression itself)" },
                    "replace_all": { "type": "boolean", "description": "Read the field at every identical occurrence in the method (default false: only the selection)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds), and it is the only check that sees a move out of `self`" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a pattern would break or the result does not compile" }
                },
                "required": ["path", "line", "character", "end_line", "end_character", "name", "type"]
            }),
        },
        McpTool {
            name: "code_generify".to_string(),
            description: "Make a parameter generic: its concrete type becomes a type parameter with the bound it must satisfy (`fn total(v: &Vec<u32>)` → `fn total<T: AsRef<[u32]>>(v: &T)`). Give the function's name position (or `symbol`), the `param` and the `bound`; `type_param` names the new parameter (default `T`, refused if the function already has one of that name). A reference in front of the type is kept. Callers do not change — the type argument is inferred — but every file that calls the function is checked against the new signature in the same overlay, so a body that uses more than the bound promises, or a caller whose type does not satisfy it, is reported before anything is written. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function" },
                    "line": { "type": "integer", "description": "1-based line of the function's name" },
                    "character": { "type": "integer", "description": "1-based column of the function's name" },
                    "param": { "type": "string", "description": "The parameter to make generic" },
                    "bound": { "type": "string", "description": "The trait bound, such as `AsRef<[u32]>` or `std::fmt::Display + Clone`" },
                    "type_param": { "type": "string", "description": "The new type parameter's name (default `T`)" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the new signature and the check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["param", "bound"]
            }),
        },
        McpTool {
            name: "code_invert_boolean".to_string(),
            description: "Invert a predicate: a function returning `bool` gets a new name and the opposite meaning (`is_valid` → `is_invalid`), and every caller keeps doing what it did. Give the function's name position (or `symbol`) and `new_name`. The body returns the negation of what it returned — a one-expression body is negated in place, a longer one as a block, and every `return` of the function (not of a closure or nested `fn` inside it) is negated. Every call becomes `!new_name(…)`, or loses the `!` it had, since the two cancel; a call followed by `.`, `?` or an index is parenthesized. A reference that is not a call — the function used as a value — is named, because it keeps its old meaning under the new name. A recursive predicate is refused. At a `bool` field or a `let` binding instead, the value is inverted: every read gains a `!` or loses the one it had (parenthesized when it goes on), and every write stores the negation — an assignment, the `let` initialiser, the field in a struct literal or its shorthand. A borrow, a compound assignment (`|=`), a pattern that binds it, a use in a format string, a derived `Default` or a serde derive cannot keep their meaning and block the write unless `force`; a local without a `: bool` annotation is inverted only when the analyzer says it is `bool`. Type-checked in one overlay; `verify: \"compile\"` adds `cargo check`. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function, field or variable" },
                    "line": { "type": "integer", "description": "1-based line of its name" },
                    "character": { "type": "integer", "description": "1-based column of its name" },
                    "new_name": { "type": "string", "description": "The name of the inverted predicate, field or variable" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["new_name"]
            }),
        },
        McpTool {
            name: "code_make_static".to_string(),
            description: "Turn a method that never uses `self` into an associated function, with every call site. Give the method's name position (or `symbol`). The receiver (`&self`, `&mut self`, `self`) leaves the declaration; `value.method(args)` becomes `Type::method(args)` and `Type::method(value, args)` loses its first argument. A receiver that does something when it is evaluated — a call, `?`, `.await`, a macro or an index, as in `load()?.method()` — cannot be dropped silently, so that call site is reported and nothing is written while one remains, unless `force`. A method whose body mentions `self` is refused. Type-checked in one overlay; `verify: \"compile\"` adds `cargo check`. An associated function (no `self`) moves with `to_type` instead: into that type's `impl`, with `Self` spelled out and every `Old::f` path, called or used as a value, naming the new type. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the method" },
                    "line": { "type": "integer", "description": "1-based line of the method's name" },
                    "character": { "type": "integer", "description": "1-based column of the method's name" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a receiver with effects would be dropped or the result does not compile" }
                }
            }),
        },
        McpTool {
            name: "code_convert_to_method".to_string(),
            description: "Turn an associated function into a method, with every call site: the other direction of `code_make_static`. Give the function's name position (or `symbol`). Its first parameter, whose type must be the `impl`'s own type (`T`, `&T`, `&mut T` or `Self`), becomes the receiver (`self`, `&self`, `&mut self`); the parameter's uses in the body become `self`, as the analyzer resolves them; and `Type::f(first, rest)` becomes `first.f(rest)`, with a leading `&` or `&mut` dropped because method syntax borrows by itself. Nothing is dropped or reordered — the receiver is evaluated first, as the first argument was. The function used as a value (`Type::f`) and a call inside the function itself are left as they are: a method is still reachable by its path. A function in a trait `impl` and a free function are refused. Type-checked in one overlay; `verify: \"compile\"` adds `cargo check`. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function" },
                    "line": { "type": "integer", "description": "1-based line of the function's name" },
                    "character": { "type": "integer", "description": "1-based column of the function's name" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                }
            }),
        },
        McpTool {
            name: "code_inline_parameter".to_string(),
            description: "Inline a parameter that every caller passes the same constant for: the value is bound at the top of the body (`let max: u32 = LIMIT;`), and the parameter leaves the declaration and its argument every call. Give the parameter's position. The value must mean the same in the body as at the call — a literal, a constant, or a path (`Mode::Fast`); a lowercase name may be a local of the caller and is refused, and so are calls that pass different values (each is listed). The function used as a value, or a call inside the function itself, blocks the write unless `force`. Type-checked in one overlay before anything is written. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function" },
                    "line": { "type": "integer", "description": "1-based line of the parameter's name" },
                    "character": { "type": "integer", "description": "1-based column of the parameter's name" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a reference is not a call or the result does not compile" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_extract_delegate".to_string(),
            description: "Extract a delegate (Extract Class): the fields you name leave a struct for a new helper type the struct then holds, with the methods you name that use only those fields. The struct keeps a forwarding method of the same signature for each moved method, so callers do not change; every other access to a moved field goes through the new field (`a.city` becomes `a.address.city`, found through the analyzer's references), and every struct literal builds the helper. A literal or pattern with `..` is refused. Type-checked in one overlay before anything is written. Rust only; plain structs with named fields."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the struct" },
                    "line": { "type": "integer", "description": "1-based line of the `struct` keyword" },
                    "character": { "type": "integer", "description": "1-based column on that line" },
                    "fields": { "type": "array", "items": { "type": "string" }, "description": "Fields that move into the helper" },
                    "methods": { "type": "array", "items": { "type": "string" }, "description": "Methods that move with them (may use only those fields)" },
                    "name": { "type": "string", "description": "Name of the helper type" },
                    "field": { "type": "string", "description": "Name of the field that holds the helper" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "fields", "name", "field"]
            }),
        },
        McpTool {
            name: "code_extract_trait".to_string(),
            description: "Extract a trait from the methods you name of an inherent `impl Type` block (rust-analyzer's `generate_trait_from_impl` takes every method, keeps the trait private and leaves callers in other modules without it in scope). The named methods move into `trait Name` and `impl Name for Type`; the rest stay inherent (the block goes when it empties). Doc comments go to the trait's declarations, attributes stay on the implementation, and the trait is as visible as the widest moved method. Every other file that references a moved method gets `use …::Name;`. Type-checked in one overlay before anything is written. Generic `impl` blocks and trait implementations are refused. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the `impl` block" },
                    "line": { "type": "integer", "description": "1-based line of the `impl` header (or any line inside the block)" },
                    "character": { "type": "integer", "description": "1-based column on that line" },
                    "methods": { "type": "array", "items": { "type": "string" }, "description": "Names of the methods that move into the trait" },
                    "name": { "type": "string", "description": "Name of the new trait" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "methods", "name"]
            }),
        },
        McpTool {
            name: "code_loop_to_iterator".to_string(),
            description: "Turn a `for` loop that only builds up an accumulator into an iterator chain: `let mut sum = 0; for p in prices { sum += p * 2; }` becomes `let sum: u64 = prices.into_iter().map(|p| p * 2).sum();`. Recognised: a sum from zero, a count (`if C { n += 1 }` into a `usize`), and a `Vec` built with `push`, each optionally under one `if`. The accumulator must be declared by the `let mut` just above the loop. Refused: `break`, `continue`, `return`, `?` or `.await` in the body, any other use of the accumulator, a non-empty start value. `mut` stays only when the analyzer says the variable is still changed afterwards. Type-checked before anything is written. Rust only; rust-analyzer's own `convert_for_loop_with_for_each` keeps the mutable accumulator."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that holds the loop" },
                    "line": { "type": "integer", "description": "1-based line of the `for`" },
                    "character": { "type": "integer", "description": "1-based column on that line" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_extract_function".to_string(),
            description: "Extract the selected code into a new function with the name you give, and replace every other place in the same file that has the same code (whitespace aside) with the same call. rust-analyzer's `extract_function` does the first place and decides the parameters and what is returned; each duplicate is kept only if the result type-checks with the call there, and the report says why any duplicate was left. rust-analyzer does not check borrows, so when a duplicate is replaced, `apply` runs `cargo check` on the result in a shadow of the workspace first (as `verify: \"compile\"` does) and writes only what compiles. `duplicates: false` extracts the selection alone. Rust only."
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
            description: "Wrap what a function returns in `Option` or `Result`, with every caller. Give the function's name position (or `symbol`) and `wrapper` (`option` or `result`; for `result` also `error`, the type it fails with, such as `anyhow::Error`). rust-analyzer's assist rewrites the signature and every returned value; this does the callers it leaves broken: a caller that itself returns an `Option` (or a `Result`) gets `?` after the call, and any other caller is reported with its line, because turning a `None` or an error into something else there is a decision. Nothing is written while such a caller remains, unless `force`. The whole change is type-checked in one overlay; `verify: \"compile\"` adds `cargo check`. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File that declares the function" },
                    "line": { "type": "integer", "description": "1-based line of the function's name" },
                    "character": { "type": "integer", "description": "1-based column of the function's name" },
                    "wrapper": { "type": "string", "enum": ["option", "result"], "description": "What the return type becomes wrapped in" },
                    "error": { "type": "string", "description": "For `result`: the error type, such as `anyhow::Error` or `String`" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when a caller cannot propagate or the result does not compile" }
                },
                "required": ["wrapper"]
            }),
        },
        McpTool {
            name: "code_extract_parameter".to_string(),
            description: "Promote an expression inside a function into a parameter of it, passing what the body used to say at every existing call site — so no caller changes behaviour and the next one can choose. Give the selection (path plus 1-based start and end line/character) and the parameter's name; the type comes from the analyzer when it gives one in a shape this can read, otherwise pass `type`. The parameter is added at the end of the list, keeping the list's shape, and the argument at the end of every call. `replace_all` puts the parameter in every identical occurrence inside the body rather than only the selected one. A reference that is not a call with this arity is named rather than mangled. The whole change is type-checked in one overlay before anything is written: an expression that names a local or anything private to the function it came from cannot be spelled at a call site, and that is what the check reports. Rust only."
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
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["path", "line", "character", "end_line", "end_character", "name"]
            }),
        },
        McpTool {
            name: "code_introduce_parameter_object".to_string(),
            description: "Bundle several of a function's parameters into a struct, with the body and every call site. `params` names the parameters to bundle (two or more, by the names the declaration gives them); they become fields of a new `pub struct` written directly above the function, in declaration order and with the types the declaration gave — a single lifetime is introduced when any of those types borrows. The declaration takes one parameter in place of them, every use of them in the body is rewritten to reach through it (at the positions the analyzer reports, not by text search), and the call sites are rewritten by a structural rule built from the declaration's own arity, so an argument that is a method chain or a closure survives and the unbundled arguments stay where they were. References the rule did not match are named. The whole change is type-checked in one overlay before anything is written, and `apply` is what writes it. Rust only; re-run your formatter afterwards."
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
                    "name": { "type": "string", "description": "The struct's name, UpperCamelCase (`Opts`, `SyncRequest`)" },
                    "binding": { "type": "string", "description": "What the new parameter is called in the body (default: the struct name in snake_case)" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                },
                "required": ["params", "name"]
            }),
        },
        McpTool {
            name: "code_move".to_string(),
            description: "Move a declaration (function, struct, enum, trait, const) into another module, with the imports that keep every user of it compiling. The item travels whole — signature, body, doc comment, attributes — is cut from its file and appended to the target; every file the analyzer lists as using it has its `use` rewritten (a grouped import keeps its other names) and any path-qualified reference requalified. A target file that does not exist yet is created and declared in its parent module (`pub mod x;` for a `pub` item), in the same change. Only the ordinary crate layout is understood (src/a.rs, src/a/mod.rs); a file reached through #[path] is named and left alone. The whole change is type-checked in one overlay first, so a move that would not compile — the item uses something private to the module it left, the target already has that name — is reported rather than written. Nothing is written without `apply`. Rust only; re-run your formatter afterwards."
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
                    "force": { "type": "boolean", "description": "Write even when something blocks it or it does not compile" }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_change_signature".to_string(),
            description: "Change what a function takes, with its call sites. `params` is the parameter list the function should end up with: `name` keeps the parameter declared under that name in this position, `name: Type = expression` adds one and passes `expression` at every call site, and a declared parameter that is not listed is removed. The arity and the types come from the declaration, so the rule that rewrites the call sites is built rather than guessed, and it is resolved in the declaring file's own scope, so calls match however they are spelled. What was rewritten is reconciled against the analyzer's reference list and anything it did not touch is named. Dropping a parameter the body still uses is refused with the usages. The whole change is type-checked together before it is written, and `apply` is what writes it. Renaming a parameter is `code_rename`. `returns` changes the return type and `visibility` the visibility in the same edit; every file that calls the function is type-checked against the new declaration, so a body that no longer returns the new type, or a caller that no longer fits it, is reported. Rust only; re-run your formatter afterwards."
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
                        "description": "The whole new parameter list, in order: `name` to keep, `name: Type = expression` to add; omit one to remove it. The receiver (`&self`) is never listed."
                    },
                    "returns": { "type": "string", "description": "The return type the function should have (`()` removes it). Every file that calls the function is type-checked against it" },
                    "visibility": { "type": "string", "description": "`pub`, `pub(crate)`, `pub(super)`, `pub(in path)`, or `private` to remove the visibility" },
                    "async": { "type": "boolean", "description": "`true` makes the function `async` and appends `.await` to every call; `false` removes both. A call that would await from a function that is not `async` blocks the write unless `force`. Not together with a change of the parameter order" },
                    "verify": { "type": "string", "enum": ["compile"], "description": "`compile`: also run `cargo check` on the result in a shadow of the workspace before writing it, and write only if the compiler accepts it too. Slower (seconds, not milliseconds) and it is the compiler — the analyzer's own check does not see an unresolved type or module path" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report the diff and the type check only)" },
                    "force": { "type": "boolean", "description": "Drop a parameter the body still uses, and write even when the result does not compile" }
                },
                "required": ["params"]
            }),
        },
        McpTool {
            name: "code_definition".to_string(),
            description: "Find where a symbol (function, struct, type, variable, module) is defined. Give `symbol` (its name, e.g. `WorkspaceSymbol` or `Metrics::record`) or a file position (path + 1-based line/column) of a use of it."
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
            name: "code_references".to_string(),
            description: "Find every reference to a symbol across the workspace. Give `symbol` (its name) or a file position (path + 1-based line/column)."
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
                    }
                },
                "required": ["path", "line", "character"]
            }),
        },
        McpTool {
            name: "code_impact".to_string(),
            description: "Blast radius of the uncommitted changes (or of the commits since a base ref): the functions the diff touches, every function that calls them (transitively, through the analyzer's call hierarchy) and the tests among those callers, plus the exact test command that runs only the affected tests."
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
            description: "Run the tests (optionally one filter) on the gateway and, for every failure, return a dossier: the failure output, the source around each location it mentions, the enclosing function and its callers, and the working-tree diff of that file. One call instead of test → grep → read → blame."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "filter": { "type": "string", "description": "Test name filter, as for code_test" },
                    "timeout_secs": { "type": "integer", "description": "Kill the run after this many seconds (default 3600)" }
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
            description: "Check a proposed new content for a file BEFORE writing it: the analyzer sees the proposed text as the document and reports errors and warnings. Nothing is written anywhere. Use it to catch hallucinated APIs, type errors and unresolved imports before touching the checkout."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute); may be a new file" },
                    "new_text": { "type": "string", "description": "The complete proposed content of the file" }
                },
                "required": ["path", "new_text"]
            }),
        },
        McpTool {
            name: "code_validate_edits".to_string(),
            description: "Check several proposed file contents TOGETHER before writing any of them: all edits are placed in one private analyzer overlay, then diagnostics are reported per file, so a change in one file is judged against the proposed state of the others (a changed signature and its updated callers). The change can be given as whole files (`edits`), as a unified diff (`diff`, e.g. `git diff` output: each hunk is applied in memory where it says, or where its old lines moved to, and a hunk that fits nowhere is refused by number), or as an LSP WorkspaceEdit (`workspace_edit`). `also_check` lists unchanged files that might break (callers of the edited symbols). Nothing is written anywhere."
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
                    "diff": { "type": "string", "description": "The change as a unified diff (`git diff` output), instead of `edits`" },
                    "workspace_edit": { "type": "object", "description": "The change as an LSP WorkspaceEdit (`changes` or `documentChanges`), instead of `edits`" },
                    "also_check": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Unchanged files to diagnose against the proposed edits (optional)"
                    }
                },
                "required": ["edits"]
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
                    "tail_bytes": { "type": "integer", "description": "Output kept per hypothesis (default 16384)" }
                },
                "required": ["hypotheses", "argv"]
            }),
        },
        McpTool {
            name: "code_slice".to_string(),
            description: "Return only the code a symbol depends on, instead of the files it lives in. Starting from the symbol, the analyzer's own edges are followed: the functions it calls, and the types, constants and traits its body mentions, each returned as its whole declaration with its file and line range. `depth` bounds how far the walk goes (default 2), `max_bytes` bounds the result. Use it to read an unfamiliar function without opening four files, and to hand a model the relevant tenth of a codebase rather than the whole of it. Names that resolve outside the workspace (std, dependencies) are listed, not expanded."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Name of the symbol to slice from (`Metrics::record`, `pkg.Func`); or give path/line/character" },
                    "path": { "type": "string", "description": "File path (relative to workspace or absolute)" },
                    "line": { "type": "integer", "description": "1-based line of the symbol" },
                    "character": { "type": "integer", "description": "1-based column of the symbol" },
                    "depth": { "type": "integer", "description": "How many edges to follow from the seed (default 2, 0 returns the seed alone)" },
                    "max_bytes": { "type": "integer", "description": "Stop once the slice reaches this many bytes (default 24576)" }
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
            description: "Build a compile-ready value for a type from the declaration the analyzer resolves the name to, so the fixture has every field the type has today. Fields are filled by type (0, false, String::new(), None, Vec::new(), and so on), types declared in this workspace are built field by field down to `depth`, and anything deeper or foreign falls back to `Default::default()`. With `verify` (default true) the fixture is type-checked in an in-memory overlay of the file that declares the type, so a missing field or a type without `Default` comes back as the analyzer's error instead of as a failed build; that file's imports are in scope during the check, so a fixture pasted into another module may still need them. Nothing is written. Rust only."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "The type to build (`Config`, `SliceReport`)" },
                    "path": { "type": "string", "description": "The file that declares it, when the name is ambiguous (a re-export makes a type resolve twice)" },
                    "depth": { "type": "integer", "description": "How deep to build nested workspace types before falling back to Default::default() (default 2)" },
                    "verify": { "type": "boolean", "description": "Type-check the fixture before returning it (default true)" }
                },
                "required": ["symbol"]
            }),
        },
        McpTool {
            name: "code_dead_code".to_string(),
            description: "Unreferenced functions, methods and types across the checkout, found through the analyzer's references (not text search). Exported/public symbols are counted separately unless include_exported is set; tests and entry points are skipped."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "include_exported": { "type": "boolean", "description": "Also list exported / public symbols nothing in the checkout uses" },
                    "max_files": { "type": "integer", "description": "Stop after this many source files (default 400)" }
                }
            }),
        },
        McpTool {
            name: "code_prune_orphans".to_string(),
            description: "Remove every orphan the dead-code scan finds (unreferenced, not exported, not reachable through a trait) with the analyzer's safe delete, all in one edit. The whole result is type-checked in one overlay before anything is written. Deletions that overlap another are left for the next run, and so is what these removals orphan: run it again until it finds nothing."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "max_files": { "type": "integer", "description": "Stop after this many source files (default 400)" },
                    "apply": { "type": "boolean", "description": "Write the change (default false: report what would be removed and the type check)" },
                    "force": { "type": "boolean", "description": "Write even when the result does not compile" }
                }
            }),
        },
        McpTool {
            name: "code_source".to_string(),
            description: "Read a source file that exists only on the gateway host: standard library sources, dependency registries (cargo, go mod cache, node_modules, site-packages) and SDK headers — the files that code_definition points at outside the checkout. Optionally a window of lines around one line."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path (or file:// URI) on the gateway, as returned by code_definition" },
                    "line": { "type": "integer", "description": "1-based line to centre on; omitted: the whole file (up to 2 MiB)" },
                    "context": { "type": "integer", "description": "Lines of context around `line` (default 30)" }
                },
                "required": ["path"]
            }),
        },
        McpTool {
            name: "code_outline".to_string(),
            description: "Extract the structural symbol outline (functions, structs, enums, traits, classes, methods, fields) with line numbers from a source file. Local variables are left out unless `include_locals` is set."
                .to_string(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path (relative to workspace or absolute)"
                    },
                    "max_depth": {
                        "type": "integer",
                        "description": "Maximum nesting depth to list, 1 = top-level items only (default: 3)"
                    },
                    "include_locals": {
                        "type": "boolean",
                        "description": "Also list local variables and bindings inside function bodies (default: false)"
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
    ];
    for tool in &mut tools {
        if SYMBOL_ADDRESSABLE.contains(&tool.name.as_str()) {
            relax_position_schema(&mut tool.input_schema);
        }
    }
    tools
}

/// Symbol-addressable tools accept `symbol` instead of a position: advertise the property and
/// stop requiring path/line/character, otherwise a schema-validating client cannot use the
/// name-based form at all.
fn relax_position_schema(schema: &mut serde_json::Value) {
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

/// Execute an MCP tool call against the remote gateway.
pub async fn execute_tool(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    // `symbol` instead of line/character: resolve the name through the workspace symbol
    // index, then run the tool at that position.
    let args = if SYMBOL_ADDRESSABLE.contains(&tool_name)
        && let Some(symbol) = args.get("symbol").and_then(|v| v.as_str())
        && !symbol.trim().is_empty()
    {
        let hint = args
            .get("path")
            .and_then(|v| v.as_str())
            .map(|p| resolve_file_path(workspace_root, p));
        let hit = resolve_symbol(remote, workspace_root, symbol.trim(), hint.as_deref()).await?;
        let mut owned = args.clone();
        if let Some(obj) = owned.as_object_mut() {
            obj.insert(
                "path".into(),
                serde_json::Value::String(hit.path.to_string_lossy().into_owned()),
            );
            obj.insert("line".into(), serde_json::json!(hit.line));
            obj.insert("character".into(), serde_json::json!(hit.col));
        }
        owned
    } else {
        args
    };
    // A path in a nested project of another language goes to a node that serves it (#125).
    let remote = crate::cluster::route_for_path(
        remote,
        workspace_root,
        args.get("path").and_then(|v| v.as_str()),
    )
    .await?;
    match tool_name {
        "code_symbols" => handle_symbols(remote, workspace_root, &args).await,
        "code_safe_delete" => handle_safe_delete(remote, workspace_root, &args).await,
        "code_assists" | "code_assist" => {
            handle_assists(remote, workspace_root, tool_name, &args).await
        }
        "code_check" | "code_lint" | "code_test" | "code_benchmarks" => {
            handle_check(remote, workspace_root, tool_name, &args).await
        }
        "code_rename" => handle_rename(remote, workspace_root, &args).await,
        "code_exec" => handle_exec(remote, workspace_root, &args).await,
        "code_definition" => handle_definition(remote, workspace_root, &args).await,

        "code_callers" | "code_callees" => {
            handle_callers(remote, workspace_root, tool_name, &args).await
        }
        "code_implementations" => handle_implementations(remote, workspace_root, &args).await,
        "code_supertypes" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let num = |key: &str| -> Result<u32> {
                args.get(key)
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .with_context(|| format!("Missing '{key}' argument"))
            };
            let file_path = resolve_file_path(workspace_root, path_str);
            let found = crate::supertypes::supertypes(
                remote,
                workspace_root,
                &file_path,
                num("line")?,
                num("character")?,
            )
            .await?;
            Ok(McpToolCallResult::text(found.render(workspace_root)))
        }
        "code_impact" => {
            let base = args.get("base").and_then(|v| v.as_str());
            let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(4) as usize;
            let report = crate::impact::analyze(remote, workspace_root, base, depth).await?;
            Ok(McpToolCallResult::text(report.render()))
        }
        "code_diagnose_failure" => handle_diagnose_failure(remote, workspace_root, &args).await,
        "code_diagnostics" | "code_validate_edit" => {
            handle_diagnostics(remote, workspace_root, tool_name, &args).await
        }
        "code_validate_edits" => handle_validate_edits(remote, workspace_root, &args).await,
        "code_shadow_run" => handle_shadow_run(remote, workspace_root, &args).await,
        "code_slice" => handle_slice(remote, workspace_root, &args).await,
        "code_search" => handle_search(remote, workspace_root, &args).await,
        "code_codemod" => handle_codemod(remote, workspace_root, &args).await,
        "code_schema_rename" => handle_schema_rename(remote, workspace_root, &args).await,
        "code_encapsulate_field" => handle_encapsulate_field(remote, workspace_root, &args).await,
        "code_migrate_type" => handle_migrate_type(remote, workspace_root, &args).await,
        "code_extract_field" => handle_extract_field(remote, workspace_root, &args).await,
        "code_wrap_return" => handle_wrap_return(remote, workspace_root, &args).await,
        "code_make_static" => handle_make_static(remote, workspace_root, &args).await,
        "code_inline_parameter" => handle_inline_parameter(remote, workspace_root, &args).await,
        "code_introduce_variable" => handle_introduce_variable(remote, workspace_root, &args).await,
        "code_extract_function" => handle_extract_function(remote, workspace_root, &args).await,
        "code_loop_to_iterator" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let num = |key: &str| -> Result<u32> {
                args.get(key)
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .with_context(|| format!("Missing '{key}' argument"))
            };
            let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let file_path = resolve_file_path(workspace_root, path_str);
            let done = crate::loop_to_iterator::loop_to_iterator(
                remote,
                workspace_root,
                &file_path,
                num("line")?,
                num("character")?,
                apply,
                force,
            )
            .await?;
            let text = done.render();
            Ok(if done.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_extract_trait" => handle_extract_trait(remote, workspace_root, &args).await,
        "code_extract_delegate" => {
            let path_str = args
                .get("path")
                .and_then(|v| v.as_str())
                .context("Missing 'path' argument")?;
            let list = |key: &str| -> Vec<String> {
                args.get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|m| m.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let text = |key: &str| -> Result<String> {
                args.get(key)
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .with_context(|| format!("Missing '{key}' argument"))
            };
            let num = |key: &str| -> Result<u32> {
                args.get(key)
                    .and_then(|v| v.as_u64())
                    .map(|v| v as u32)
                    .with_context(|| format!("Missing '{key}' argument"))
            };
            let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let file_path = resolve_file_path(workspace_root, path_str);
            let done = crate::extract_delegate::extract_delegate(
                remote,
                workspace_root,
                &file_path,
                num("line")?,
                num("character")?,
                &list("fields"),
                &list("methods"),
                &text("name")?,
                &text("field")?,
                apply,
                force,
            )
            .await?;
            let out = done.render();
            Ok(if done.diagnostics.is_empty() {
                McpToolCallResult::text(out)
            } else {
                McpToolCallResult::error(out)
            })
        }
        "code_convert_to_method" => handle_convert_to_method(remote, workspace_root, &args).await,
        "code_invert_boolean" => handle_invert_boolean(remote, workspace_root, &args).await,
        "code_generify" => handle_generify(remote, workspace_root, &args).await,
        "code_extract_parameter" => handle_extract_parameter(remote, workspace_root, &args).await,
        "code_introduce_parameter_object" => {
            handle_introduce_parameter_object(remote, workspace_root, &args).await
        }
        "code_move" => handle_move(remote, workspace_root, &args).await,
        "code_move_module" => handle_move_module(remote, workspace_root, &args).await,
        "code_move_method" => handle_move_method(remote, workspace_root, &args).await,
        "code_change_signature" => handle_change_signature(remote, workspace_root, &args).await,
        "code_generate_fixture" => handle_generate_fixture(remote, workspace_root, &args).await,
        "code_dead_code" => handle_dead_code(remote, workspace_root, &args).await,
        "code_prune_orphans" => {
            let max_files = args
                .get("max_files")
                .and_then(|v| v.as_u64())
                .unwrap_or(400) as usize;
            let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            let pruned =
                crate::prune::prune_orphans(remote, workspace_root, max_files, apply, force)
                    .await?;
            let text = pruned.render();
            Ok(if pruned.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            })
        }
        "code_source" => handle_source(remote, &args).await,
        "code_references" => handle_references(remote, workspace_root, &args).await,

        "code_outline" => handle_outline(remote, workspace_root, &args).await,

        "code_hover" | "code_type_at" => handle_hover(remote, workspace_root, &args).await,

        "code_status" => handle_status(remote).await,

        "code_sync" => handle_sync(remote, workspace_root, args).await,

        unknown => Ok(McpToolCallResult::error(format!("Unknown tool: {unknown}"))),
    }
}

async fn handle_sync(
    remote: SocketAddr,
    workspace_root: &Path,
    args: serde_json::Value,
) -> Result<McpToolCallResult> {
    let subpath = args.get("path").and_then(|v| v.as_str()).map(Path::new);
    let deltas = scan_workspace_files(workspace_root, subpath)?;
    let file_count = deltas.len();
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    let req = SyncRequest {
        client_workspace_root: workspace_root.to_string_lossy().to_string(),
        files: deltas,
        clean_others: false,
        base_workspace_name: None,
    };
    framed.send(WireMessage::SyncRequest(req)).await?;
    if let Some(msg_res) = framed.next().await {
        match msg_res? {
            WireMessage::SyncResponse(resp) => {
                let kb = (resp.bytes_transferred as f64) / 1024.0;
                let info = format!(
                    "⚡ Fast-Sync Completed in {}ms\n\
                             • Files scanned: {file_count}\n\
                             • Files updated: {}\n\
                             • Files deleted: {}\n\
                             • Data transferred: {kb:.1} KB\n\
                             • Remote workspace: {}",
                    resp.duration_ms,
                    resp.files_updated,
                    resp.files_deleted,
                    resp.server_workspace_root
                );
                Ok(McpToolCallResult::text(info))
            }
            other => Ok(McpToolCallResult::error(format!(
                "Unexpected response: {other:?}"
            ))),
        }
    } else {
        Ok(McpToolCallResult::error(
            "Gateway closed connection without sync response",
        ))
    }
}

async fn handle_status(remote: SocketAddr) -> Result<McpToolCallResult> {
    let start = std::time::Instant::now();
    let stream = TcpStream::connect(remote)
        .await
        .with_context(|| format!("Failed to connect to gateway at {remote}"))?;
    let _ = stream.set_nodelay(true);
    let rtt = start.elapsed();
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed.send(WireMessage::StatusRequest).await?;
    if let Some(msg_res) = framed.next().await {
        match msg_res? {
            WireMessage::StatusResponse(resp) => {
                let hours = resp.uptime_seconds / 3600;
                let minutes = (resp.uptime_seconds % 3600) / 60;
                let seconds = resp.uptime_seconds % 60;
                let mem = resp.memory_rss_mb().unwrap_or(0.0);

                let info = format!(
                    "⚡ prod-code Gateway Status\n\
                             • Address: {remote} ({rtt:.2?} RTT)\n\
                             • Server PID: {}\n\
                             • Uptime: {hours}h {minutes}m {seconds}s\n\
                             • Memory RSS: {mem:.2} MB\n\
                             • Active Sessions: {}\n\
                             • Loaded Workspaces: {}\n\
                             • Queries Handled: {} (in-flight: {})\n\
                             • Engines: {}\n\
                             • Status: HEALTHY",
                    resp.server_pid,
                    resp.active_sessions,
                    resp.loaded_workspaces,
                    resp.total_queries,
                    resp.active_queries,
                    resp.detected_engines.join(", ")
                );
                Ok(McpToolCallResult::text(info))
            }
            other => Ok(McpToolCallResult::error(format!(
                "Unexpected response from gateway: {other:?}"
            ))),
        }
    } else {
        Ok(McpToolCallResult::error(
            "Gateway closed connection without status response",
        ))
    }
}

async fn handle_hover(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/hover",
        params,
    )
    .await?;
    if let Some(contents) = res.get("contents") {
        if let Some(val) = contents.get("value").and_then(|v| v.as_str()) {
            return Ok(McpToolCallResult::text(val));
        } else if let Some(arr) = contents.as_array() {
            let text = arr
                .iter()
                .filter_map(|i| i.get("value").and_then(|v| v.as_str()))
                .collect::<Vec<_>>()
                .join("\n\n");
            return Ok(McpToolCallResult::text(text));
        }
    }
    Ok(McpToolCallResult::text("No hover information available."))
}

async fn handle_outline(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/documentSymbol",
        params,
    )
    .await?;
    let max_depth = args
        .get("max_depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(3)
        .max(1) as usize;
    let include_locals = args
        .get("include_locals")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(McpToolCallResult::text(render_outline(
        &res,
        path_str,
        max_depth,
        include_locals,
        "pass include_locals: true",
    )))
}

/// The start and end line (0-based) of a symbol from `textDocument/documentSymbol`.
fn symbol_lines(sym: &serde_json::Value) -> (u64, u64) {
    let range = sym
        .get("range")
        .or_else(|| sym.get("location").and_then(|l| l.get("range")));
    let at = |edge: &str| {
        range
            .and_then(|r| r.get(edge))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
    };
    let start = at("start").unwrap_or(0);
    (start, at("end").unwrap_or(start).max(start))
}

/// A file's outline from its `textDocument/documentSymbol` answer, for the MCP tool and the
/// CLI alike. A variable inside a function or method is a local and is left out unless
/// `include_locals`; a top-level `static`, which the analyzer reports with the same kind, is not
/// inside one and stays. `hint` says how to list the locals anyway.
pub fn render_outline(
    res: &serde_json::Value,
    path: &str,
    max_depth: usize,
    include_locals: bool,
    hint: &str,
) -> String {
    let mut out = String::new();
    if let Some(arr) = res.as_array() {
        out.push_str(&format!("Outline for {path}:\n"));
        let bodies: Vec<(u64, u64)> = arr
            .iter()
            .filter(|s| matches!(s.get("kind").and_then(|k| k.as_u64()), Some(6 | 12)))
            .map(symbol_lines)
            .collect();
        let mut skipped_locals = 0usize;
        for sym in arr {
            let name = sym.get("name").and_then(|n| n.as_str()).unwrap_or("");
            let kind = sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            // Locals (LSP kind 13, Variable, inside a body) are noise for a structural
            // outline: a 2000-line file lists hundreds of them.
            let (line, _) = symbol_lines(sym);
            let local = kind == 13 && bodies.iter().any(|(s, e)| *s < line && line <= *e);
            if local && !include_locals {
                skipped_locals += 1;
                continue;
            }
            // Depth from the container chain the gateway reports ("a > b > c").
            let depth = sym
                .get("containerName")
                .and_then(|c| c.as_str())
                .filter(|c| c.contains(" > "))
                .map(|c| c.split(" > ").count() + 1)
                .unwrap_or(1);
            if depth > max_depth {
                continue;
            }
            let kind_str = match kind {
                2 => "Module",
                5 => "Class",
                6 => "Method",
                8 => "Field",
                9 => "Constructor",
                10 => "Enum",
                11 => "Interface",
                12 => "Function",
                13 => "Variable",
                14 => "Constant",
                22 => "EnumMember",
                23 => "Struct",
                _ => "Symbol",
            };
            out.push_str(&format!("  [{kind_str}] {name} (line {})\n", line + 1));
        }
        if skipped_locals > 0 {
            out.push_str(&format!(
                "  ({skipped_locals} local variable(s) hidden; {hint} to list them)\n"
            ));
        }
    } else {
        out.push_str("No outline symbols available.");
    }
    out.trim_end().to_string()
}

async fn handle_references(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let include_decl = args
        .get("include_declarations")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "context": { "includeDeclaration": include_decl }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/references",
        params,
    )
    .await?;
    let mut out = String::new();
    if let Some(arr) = res.as_array() {
        if arr.is_empty() {
            out.push_str("No references found.");
        } else {
            out.push_str(&format!("Found {} reference(s):\n", arr.len()));
            for loc in arr {
                let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
                let start_line = loc
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = loc
                    .get("range")
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                out.push_str(&format!("  • {uri}:{start_line}:{start_col}\n"));
            }
        }
    } else {
        out.push_str("No references found.");
    }
    Ok(McpToolCallResult::text(out.trim_end()))
}

async fn handle_source(remote: SocketAddr, args: &serde_json::Value) -> Result<McpToolCallResult> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let path = crate::remote_fs::uri_to_path(path);
    let line = args.get("line").and_then(|v| v.as_u64()).map(|l| l as u32);
    let context = args.get("context").and_then(|v| v.as_u64()).unwrap_or(30) as u32;
    let (bytes, truncated) = crate::remote_fs::read_remote_file(remote, &path, 0).await?;
    let text = String::from_utf8_lossy(&bytes);
    let mut out = match line {
        Some(line) => crate::remote_fs::snippet(&text, line, context),
        None => text.into_owned(),
    };
    if truncated {
        out.push_str("\n[truncated at 2 MiB]");
    }
    Ok(McpToolCallResult::text(out))
}

async fn handle_dead_code(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let include_exported = args
        .get("include_exported")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let max_files = args
        .get("max_files")
        .and_then(|v| v.as_u64())
        .unwrap_or(400) as usize;
    let report =
        crate::dead_code::find_dead_code(remote, workspace_root, include_exported, max_files)
            .await?;
    Ok(McpToolCallResult::text(report.render()))
}

async fn handle_generate_fixture(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let symbol = args
        .get("symbol")
        .and_then(|v| v.as_str())
        .context("Missing 'symbol' argument")?;
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::fixture::DEFAULT_DEPTH as u64) as u32;
    let verify = args.get("verify").and_then(|v| v.as_bool()).unwrap_or(true);
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let fixture = crate::fixture::generate(
        remote,
        workspace_root,
        symbol,
        depth,
        verify,
        hint.as_deref(),
    )
    .await?;
    let clean = fixture.diagnostics.is_empty();
    let text = fixture.render();
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_change_signature(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let specs = args
        .get("params")
        .and_then(|v| v.as_array())
        .context("Missing 'params' argument: the parameter list the function should end up with")?;
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        let spec = spec
            .as_str()
            .context("every entry of `params` is a string: `name`, or `name: Type = expression`")?;
        params.push(crate::signature::parse_param(spec)?);
    }
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let modifiers = crate::signature::Modifiers {
        returns: args
            .get("returns")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        visibility: args
            .get("visibility")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        asyncness: args.get("async").and_then(|v| v.as_bool()),
    };
    let mut change = crate::signature::change_with(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &params,
        &modifiers,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify {
        let files = change.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                change.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        change.applied = true;
    }
    let clean = change.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = change.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_move(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: the target module's file")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let target = resolve_file_path(workspace_root, to);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let mut moved = crate::move_item::move_item(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &target,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify {
        let files = moved.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                moved.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        moved.applied = true;
    }
    let clean = moved.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = moved.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_introduce_parameter_object(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new struct is called")?;
    let specs = args
        .get("params")
        .and_then(|v| v.as_array())
        .context("Missing 'params' argument: the parameters to bundle, by name")?;
    let mut params = Vec::with_capacity(specs.len());
    for spec in specs {
        params.push(
            spec.as_str()
                .context("every entry of `params` is a parameter name")?
                .to_string(),
        );
    }
    let binding = args
        .get("binding")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| crate::fixture::snake_case(name));
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let mut done = crate::parameter_object::introduce(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        &params,
        name,
        &binding,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_extract_parameter(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new parameter is called")?;
    let ty = args.get("type").and_then(|v| v.as_str());
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let mut done = crate::extract_parameter::extract(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        ty,
        replace_all,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_extract_field(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument: what the new field is called")?;
    let ty = args.get("type").and_then(|v| v.as_str());
    let init = args.get("init").and_then(|v| v.as_str());
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::extract_field::extract(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        ty,
        init,
        replace_all,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify && (done.blocked.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_generify(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument (or `symbol`)"))
    };
    let param = args
        .get("param")
        .and_then(|v| v.as_str())
        .context("Missing 'param' argument: the parameter to make generic")?;
    let bound = args
        .get("bound")
        .and_then(|v| v.as_str())
        .context("Missing 'bound' argument: the trait the type must satisfy")?;
    let type_param = args
        .get("type_param")
        .and_then(|v| v.as_str())
        .unwrap_or("T");
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::generify::generify(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        param,
        bound,
        type_param,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_invert_boolean(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument (or `symbol`)"))
    };
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument: the name of the inverted predicate")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::invert_boolean::invert(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        new_name,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_make_static(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument (or `symbol`)"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::make_static::make_static(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify && (done.blocked.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_convert_to_method(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument (or `symbol`)"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::to_method::convert_to_method(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_inline_parameter(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::inline_parameter::inline_parameter(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        apply,
        force,
    )
    .await?;
    let text = done.render(6000);
    Ok(
        if done.diagnostics.is_empty() && done.unmatched.is_empty() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        },
    )
}

async fn handle_extract_trait(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let methods: Vec<String> = args
        .get("methods")
        .and_then(|v| v.as_array())
        .context("Missing 'methods' argument")?
        .iter()
        .filter_map(|m| m.as_str().map(str::to_string))
        .collect();
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::extract_trait::extract_trait(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        &methods,
        name,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_move_method(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file = resolve_file_path(workspace_root, path_str);
    let (line, character) = (num("line")?, num("character")?);
    let done = match (
        args.get("to_param").and_then(|v| v.as_str()),
        args.get("to_type").and_then(|v| v.as_str()),
    ) {
        (Some(to_param), None) => {
            crate::move_method::move_method(
                remote,
                workspace_root,
                &file,
                line,
                character,
                to_param,
                apply,
                force,
            )
            .await?
        }
        (None, Some(to_type)) => {
            crate::move_method::move_associated_function(
                remote,
                workspace_root,
                &file,
                line,
                character,
                to_type,
                apply,
                force,
            )
            .await?
        }
        _ => anyhow::bail!(
            "give `to_param` (a method: the parameter whose type it moves to) or `to_type` (an \
             associated function: the type it moves to), one of them"
        ),
    };
    let text = done.render(8000);
    Ok(if done.diagnostics.is_empty() && done.blocked.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_move_module(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument: the module's file")?;
    let to_str = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: where the module's file goes")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let mut done = crate::move_module::move_module(
        remote,
        workspace_root,
        &resolve_file_path(workspace_root, path_str),
        &resolve_file_path(workspace_root, to_str),
    )
    .await?;
    let gate = if verify {
        Some(
            compile_gate(
                remote,
                workspace_root,
                &done.rewritten,
                done.diagnostics.is_empty(),
                false,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    let compiles = gate.as_ref().is_none_or(|g| g.passed);
    // The gate only judges: the move also deletes the files it left, which only `write` does.
    if apply && (compiles || force) {
        done.write(force)?;
    }
    let clean = done.diagnostics.is_empty() && compiles;
    let mut text = done.render(8000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
        if apply && !compiles && !force {
            text.push_str("\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n");
        }
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_extract_function(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let duplicates = args
        .get("duplicates")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let flag = |key: &str| args.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
    let apply = flag("apply");
    let force = flag("force");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::extract_function::extract_function(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        duplicates,
        flag("parameterize"),
        flag("other_files"),
    )
    .await?;
    // rust-analyzer does not check borrows: a duplicate whose call moves a value the code after
    // it still uses type-checks and does not compile. So the compiler sees any such result.
    let verify =
        args.get("verify").and_then(|v| v.as_str()) == Some("compile") || done.replaced() > 0;
    let gate = if verify {
        Some(
            compile_gate(
                remote,
                workspace_root,
                &done.rewritten,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        if apply {
            done.write(force)?;
        }
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render();
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_introduce_variable(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .context("Missing 'name' argument")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument"))
    };
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::introduce_variable::introduce_variable(
        remote,
        workspace_root,
        &file_path,
        (num("line")?, num("character")?),
        (num("end_line")?, num("end_character")?),
        name,
        apply,
        force,
    )
    .await?;
    let text = done.render();
    Ok(if done.diagnostics.is_empty() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_wrap_return(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let num = |key: &str| -> Result<u32> {
        args.get(key)
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
            .with_context(|| format!("Missing '{key}' argument (or `symbol`)"))
    };
    let wrapper = crate::wrap_return::Wrapper::parse(
        args.get("wrapper")
            .and_then(|v| v.as_str())
            .context("Missing 'wrapper' argument: `option` or `result`")?,
    )?;
    let error = args.get("error").and_then(|v| v.as_str());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::wrap_return::wrap(
        remote,
        workspace_root,
        &file_path,
        num("line")?,
        num("character")?,
        wrapper,
        error,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify && (done.blocked.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_encapsulate_field(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let by_value = args.get("by_value").and_then(|v| v.as_bool());
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let file_path = resolve_file_path(workspace_root, path_str);
    let mut done = crate::encapsulate_field::encapsulate(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        by_value,
        apply && !verify,
        force,
    )
    .await?;
    let gate = if verify && (done.blocked.is_empty() || force) {
        let files = done.rewritten.clone();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty()
        && done.blocked.is_empty()
        && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_schema_rename(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let field = args
        .get("field")
        .and_then(|v| v.as_str())
        .context("Missing 'field' argument")?;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument")?;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let scope = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let verify = args.get("verify").and_then(|v| v.as_str()) == Some("compile");
    let repos = match args.get("repos") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .map(|v| {
                let path = v
                    .as_str()
                    .with_context(|| format!("repos takes paths, got {v}"))?;
                let path = resolve_file_path(workspace_root, path);
                std::fs::canonicalize(&path)
                    .with_context(|| format!("repository {} cannot be read", path.display()))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("repos takes a list of paths, got {other}"),
    };
    if !repos.is_empty() {
        anyhow::ensure!(
            scope.is_none() && !verify,
            "`path` and `verify` narrow or check one repository; drop them to rename across `repos`"
        );
        let mut roots = vec![workspace_root.to_path_buf()];
        roots.extend(repos);
        let done = crate::schema::rename_across(remote, &roots, field, to, apply, force).await?;
        let text = done.render(6000);
        return Ok(if done.clean() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let mut done = crate::schema::rename(
        remote,
        workspace_root,
        field,
        to,
        apply && !verify,
        force,
        scope.as_deref(),
    )
    .await?;
    let gate = if verify {
        let files = done
            .rewritten
            .iter()
            .map(|(p, t)| (p.to_string_lossy().into_owned(), t.clone()))
            .collect::<Vec<_>>();
        Some(
            compile_gate(
                remote,
                workspace_root,
                &files,
                done.diagnostics.is_empty(),
                apply,
                force,
            )
            .await?,
        )
    } else {
        None
    };
    if gate.as_ref().is_some_and(|g| g.applied) {
        done.applied = true;
    }
    let clean = done.diagnostics.is_empty() && gate.as_ref().is_none_or(|g| g.passed);
    let mut text = done.render(6000);
    if let Some(gate) = &gate {
        text.push_str(&gate.text);
    }
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_codemod(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let rule = args
        .get("rule")
        .and_then(|v| v.as_str())
        .context("Missing 'rule' argument")?;
    if !rule.contains("==>>") {
        return Ok(McpToolCallResult::error(
            "a rule is `pattern ==>> replacement`, for example `$a.unwrap() ==>> $a.expect(\"invariant\")`"
                .to_string(),
        ));
    }
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    // `path` restricts the rewrite to one file and doubles as the resolve context.
    // Without it the rewrite covers the workspace, which is correct and slow.
    let scope = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let context = match scope.clone() {
        Some(p) => p,
        None => representative_source_file(workspace_root)
            .context("no source file found to resolve the rule against; pass `path`")?,
    };
    let uri = Url::from_file_path(&context)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", context))?
        .to_string();
    let edit = execute_lsp_query(
        remote,
        workspace_root,
        &context,
        "prodCode/structuralReplace",
        serde_json::json!({
            "rule": rule,
            "scope": scope.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "textDocument": { "uri": uri },
            "position": { "line": 0, "character": 0 },
        }),
    )
    .await?;
    let rewritten = rewritten_files(&edit);
    if rewritten.is_empty() {
        return Ok(McpToolCallResult::text(format!(
            "`{rule}` matches nothing{}",
            match &scope {
                Some(p) => format!(" in {}", p.display()),
                None => " in this workspace".to_string(),
            }
        )));
    }
    let mut text = format!("`{rule}`\n");
    let mut changed_lines = 0usize;
    let mut body = String::new();
    for (path, new_text) in &rewritten {
        let rel = std::path::Path::new(path)
            .strip_prefix(workspace_root)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.clone());
        let old_text = crate::refactor::text_before_apply(Path::new(path));
        let diff = similar::TextDiff::from_lines(&old_text, new_text);
        let file_changed = diff
            .iter_all_changes()
            .filter(|c| c.tag() != similar::ChangeTag::Equal)
            .count();
        changed_lines += file_changed;
        body.push_str(
            &diff
                .unified_diff()
                .context_radius(2)
                .header(&format!("a/{rel}"), &format!("b/{rel}"))
                .to_string(),
        );
    }
    text.push_str(&format!(
        "{} changed line(s) in {} file(s)\n\n",
        changed_lines,
        rewritten.len()
    ));
    const MAX_DIFF: usize = 6000;
    if body.len() > MAX_DIFF {
        let cut: String = body.chars().take(MAX_DIFF).collect();
        text.push_str(&cut);
        text.push_str("\n… diff truncated\n");
    } else {
        text.push_str(&body);
    }
    if apply {
        let written = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        text.push_str(&format!(
            "\n[applied to {} file(s): {}]\n",
            written.len(),
            written.join(", ")
        ));
    } else {
        text.push_str("\nnothing was written; pass `apply: true` to make these edits\n");
    }
    Ok(McpToolCallResult::text(text.trim_end().to_string()))
}

async fn handle_search(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .context("Missing 'query' argument")?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let subpath = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p))
        .and_then(|p| crate::exec::subdir_of(workspace_root, &p));
    let resp =
        crate::search::search(remote, workspace_root, query, limit, subpath.as_deref()).await?;
    Ok(McpToolCallResult::text(crate::search::render(&resp, query)))
}

async fn handle_slice(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or pass 'symbol')")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or pass 'symbol')")? as u32;
    let character = args.get("character").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let depth = args
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::slice::DEFAULT_DEPTH as u64) as u32;
    let max_bytes = args
        .get("max_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(crate::slice::DEFAULT_MAX_BYTES as u64) as usize;
    let file_path = resolve_file_path(workspace_root, path_str);
    let report = crate::slice::slice(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        depth,
        max_bytes,
    )
    .await?;
    Ok(McpToolCallResult::text(report.render()))
}

async fn handle_shadow_run(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let argv: Vec<String> = args
        .get("argv")
        .and_then(|v| v.as_array())
        .context("Missing 'argv' argument")?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    if argv.is_empty() {
        return Ok(McpToolCallResult::error("'argv' is empty".to_string()));
    }
    let specs = crate::shadow::parse_specs(workspace_root, args, None)?;
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let parallel = args.get("parallel").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let tail_bytes = args
        .get("tail_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(16 * 1024) as usize;
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let subdir = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p))
        .and_then(|p| crate::exec::subdir_of(workspace_root, &p));
    let outcome = crate::shadow::run_shadow(
        remote,
        workspace_root,
        subdir.as_deref(),
        &specs,
        argv.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        parallel,
        tail_bytes,
    )
    .await?;
    let applied = match (apply, outcome.winner) {
        (true, Some(i)) => Some(crate::shadow::apply_hypothesis(workspace_root, &specs[i])?),
        _ => None,
    };
    let text = crate::shadow::render_report(&outcome, &argv, applied.as_deref(), 2000);
    Ok(if outcome.winner.is_some() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_validate_edits(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    // A patch or a WorkspaceEdit becomes whole files first: the overlay takes files.
    let mut notes = String::new();
    let edits: Vec<(std::path::PathBuf, String)> =
        if let Some(diff) = args.get("diff").and_then(|v| v.as_str()) {
            let patched = crate::patch::apply(workspace_root, diff)?;
            for gone in &patched.deleted {
                notes.push_str(&format!(
                    "{} is deleted by the diff; what still uses it is not checked here\n",
                    gone.strip_prefix(workspace_root).unwrap_or(gone).display()
                ));
            }
            patched.texts
        } else if let Some(edit) = args.get("workspace_edit") {
            let (planned, moves) = crate::refactor::planned_texts(workspace_root, edit)?;
            if moves {
                notes.push_str(
                "the edit also creates, renames or deletes files; those parts are not checked\n",
            );
            }
            planned
        } else {
            args.get("edits")
                .and_then(|v| v.as_array())
                .context("Missing 'edits' argument (or `diff`, or `workspace_edit`)")?
                .iter()
                .map(|e| {
                    let path = e
                        .get("path")
                        .and_then(|v| v.as_str())
                        .context("edit without 'path'")?;
                    let text = e
                        .get("new_text")
                        .and_then(|v| v.as_str())
                        .with_context(|| format!("edit for {path} without 'new_text'"))?;
                    Ok((resolve_file_path(workspace_root, path), text.to_string()))
                })
                .collect::<Result<_>>()?
        };
    if edits.is_empty() {
        return Ok(McpToolCallResult::error(
            "the change touches no file that can be checked".to_string(),
        ));
    }
    let also_check: Vec<std::path::PathBuf> = args
        .get("also_check")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|p| resolve_file_path(workspace_root, p))
                .collect()
        })
        .unwrap_or_default();
    let reports =
        crate::diagnostics::validate_texts(remote, workspace_root, &edits, &also_check).await?;
    let errors: usize = reports.iter().map(|r| r.errors).sum();
    let warnings: usize = reports.iter().map(|r| r.warnings).sum();
    let mut text = format!(
        "{} file(s) checked together: {errors} error(s), {warnings} warning(s)\n{notes}",
        reports.len()
    );
    for report in &reports {
        text.push_str(&report.render());
    }
    let text = text.trim_end().to_string();
    Ok(if errors == 0 {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_diagnostics(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let file_path = resolve_file_path(workspace_root, path_str);
    let report = match args.get("new_text").and_then(|v| v.as_str()) {
        Some(text) if tool_name == "code_validate_edit" => {
            crate::diagnostics::validate_text(remote, workspace_root, &file_path, text).await?
        }
        _ if tool_name == "code_validate_edit" => {
            return Ok(McpToolCallResult::error(
                "Missing 'new_text' argument".to_string(),
            ));
        }
        _ => crate::diagnostics::diagnostics(remote, workspace_root, &file_path).await?,
    };
    let text = report.render();
    Ok(if report.ok() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_diagnose_failure(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let filter = args.get("filter").and_then(|v| v.as_str());
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let report = crate::dossier::diagnose(remote, workspace_root, filter, timeout_secs).await?;
    let text = report.render();
    Ok(
        if report.tests_failed == 0 && report.build_errors.is_empty() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        },
    )
}

async fn handle_implementations(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/implementation",
        params,
    )
    .await?;
    let arr = match &res {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![res.clone()],
        _ => Vec::new(),
    };
    if arr.is_empty() {
        return Ok(McpToolCallResult::text(
            "No implementations found.".to_string(),
        ));
    }
    let mut out = format!("Found {} implementation(s):\n", arr.len());
    for loc in &arr {
        let uri = loc.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start = loc.get("range").and_then(|r| r.get("start"));
        let l = start
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let c = start
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        out.push_str(&format!("  • {uri}:{l}:{c}\n"));
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}

async fn handle_callers(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let incoming = tool_name == "code_callers";
    let depth = args.get("depth").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
    let file_path = resolve_file_path(workspace_root, path_str);
    let tree = crate::call_tree::call_tree(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        incoming,
        depth,
    )
    .await?;
    Ok(McpToolCallResult::text(match tree {
        Some(tree) => tree.render(),
        None => format!("No function at {path_str}:{line}:{character}."),
    }))
}

async fn handle_definition(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let res = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/definition",
        params,
    )
    .await?;
    let mut out = String::new();
    if let Some(arr) = res.as_array() {
        if arr.is_empty() {
            out.push_str("No definition found.");
        } else {
            for (i, loc) in arr.iter().enumerate() {
                let uri = loc
                    .get("uri")
                    .or_else(|| loc.get("targetUri"))
                    .and_then(|u| u.as_str())
                    .unwrap_or("");
                let range = loc.get("range").or_else(|| loc.get("targetSelectionRange"));
                let start_line = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .unwrap_or(0)
                    + 1;
                let start_col = range
                    .and_then(|r| r.get("start"))
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .unwrap_or(0)
                    + 1;
                if i > 0 {
                    out.push('\n');
                }
                out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
                // Outside the checkout the file exists only on the gateway: include
                // the lines around the definition so the agent can read it.
                let path = crate::remote_fs::uri_to_path(uri);
                if i < 3 && crate::remote_fs::is_external(workspace_root, &path) {
                    match crate::remote_fs::read_remote_file(remote, &path, 0).await {
                        Ok((bytes, _)) => {
                            let text = String::from_utf8_lossy(&bytes);
                            out.push('\n');
                            out.push_str(&crate::remote_fs::snippet(&text, start_line as u32, 8));
                        }
                        Err(e) => {
                            out.push_str(&format!("\n   (external source not readable: {e})"))
                        }
                    }
                }
            }
        }
    } else if let Some(obj) = res.as_object() {
        let uri = obj.get("uri").and_then(|u| u.as_str()).unwrap_or("");
        let start_line = obj
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|l| l.as_u64())
            .unwrap_or(0)
            + 1;
        let start_col = obj
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("character"))
            .and_then(|c| c.as_u64())
            .unwrap_or(0)
            + 1;
        out.push_str(&format!("📍 Definition: {uri}:{start_line}:{start_col}"));
    } else {
        out.push_str("No definition found.");
    }
    Ok(McpToolCallResult::text(out))
}

async fn handle_exec(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let argv: Vec<String> = args
        .get("argv")
        .and_then(|v| v.as_array())
        .context("Missing 'argv' argument")?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let tail_bytes = args
        .get("tail_bytes")
        .and_then(|v| v.as_u64())
        .unwrap_or(16 * 1024) as usize;
    let mut tail = crate::exec::TailBuffer::new(tail_bytes);
    let subdir = args
        .get("cwd")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p))
        .and_then(|p| crate::exec::subdir_of(workspace_root, &p));
    let outcome = crate::exec::run_remote(
        remote,
        workspace_root,
        subdir.as_deref(),
        argv.clone(),
        vec![("CARGO_TERM_COLOR".to_string(), "never".to_string())],
        timeout_secs,
        true,
        |_, data| tail.push(data),
    )
    .await?;
    let exit = outcome.exit;
    let status = match (&exit.error, exit.timed_out, exit.exit_code) {
        (Some(err), _, _) => format!("failed to start: {err}"),
        (None, true, _) => "timed out".to_string(),
        (None, false, Some(code)) => format!("exit code {code}"),
        (None, false, None) => "killed by signal".to_string(),
    };
    let mut text = format!(
        "$ {}\n[{status} in {:.1}s{} on {}; {} bytes of output{}]\n",
        argv.join(" "),
        exit.duration_ms as f64 / 1000.0,
        exit.usage
            .map(|u| format!(" ({})", u.render()))
            .unwrap_or_default(),
        exit.server_workspace_root,
        tail.total,
        if tail.total > tail_bytes {
            ", tail shown"
        } else {
            ""
        }
    );
    if !outcome.pulled_files.is_empty() {
        text.push_str(&format!(
            "[{} file(s) changed by the command were written back: {}]\n",
            outcome.pulled_files.len(),
            outcome.pulled_files.join(", ")
        ));
    }
    text.push_str(&tail.text());
    Ok(if matches!(exit.exit_code, Some(0)) {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

async fn handle_rename(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let new_name = args
        .get("new_name")
        .and_then(|v| v.as_str())
        .context("Missing 'new_name' argument")?
        .to_string();
    let file_path = resolve_file_path(workspace_root, path_str);
    if args
        .get("accessors")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        return rename_with_accessors(
            remote,
            workspace_root,
            &file_path,
            line,
            character,
            &new_name,
            force,
        )
        .await;
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        "newName": new_name
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "textDocument/rename",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if edit.is_null() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    // The analyzer computes the edit; it does not check that the result compiles. A new name
    // that is already declared in the same scope is renamed into a second definition (#98), so
    // the result is checked in the overlay like every other write, and refused if it breaks.
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let (mut planned, moves_files) = crate::refactor::planned_texts(workspace_root, &edit)?;
    // The old name in comments and test names, in every file the rename touches.
    let comments = args
        .get("comments")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut mentioned = crate::rename_mentions::Mentions::default();
    if comments {
        if moves_files {
            return Ok(McpToolCallResult::error(
                "`comments` is not supported with a rename that moves files; rename first, then \
                 run it again at the new name"
                    .to_string(),
            ));
        }
        let text = std::fs::read_to_string(&file_path).unwrap_or_default();
        let at = crate::signature::offset_of(&text, line, character).unwrap_or(0);
        let start = text[..at]
            .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map_or(0, |i| i + 1);
        let old: String = text[start..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !planned.iter().any(|(p, _)| *p == file_path) {
            planned.push((file_path.clone(), text.clone()));
        }
        for (_, t) in planned.iter_mut() {
            let (rewritten, found) = crate::rename_mentions::rewrite(t, &old, &new_name);
            mentioned.comments += found.comments;
            mentioned.tests.extend(found.tests);
            *t = rewritten;
        }
    }
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename to `{new_name}` refused: the result does not compile ({} error(s)); nothing \
             was written. If `{new_name}` is already declared in that scope, pick another name; \
             pass `force: true` to write it anyway:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    // With `comments` the texts are no longer the analyzer's edit alone: write them whole.
    let touched = if comments {
        let files: std::collections::BTreeMap<std::path::PathBuf, String> = planned
            .into_iter()
            .filter(|(p, t)| std::fs::read_to_string(p).map(|o| o != *t).unwrap_or(true))
            .collect();
        crate::refactor::apply_workspace_edit(
            workspace_root,
            &crate::signature::whole_file_edit(&files),
        )?
    } else {
        crate::refactor::apply_workspace_edit(workspace_root, &edit)?
    };
    let mut text = format!(
        "renamed to `{new_name}`; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    );
    if comments {
        text.push_str(&format!(
            "\n\nin comments: {} mention(s) of the old name replaced",
            mentioned.comments
        ));
        for (from, to) in &mentioned.tests {
            text.push_str(&format!("\ntest renamed: `{from}` -> `{to}`"));
        }
    }
    if !errors.is_empty() {
        text.push_str(&format!(
            "\n\nwritten with `force`, although the analyzer reports {} error(s):\n  {}",
            errors.len(),
            errors.join("\n  ")
        ));
    }
    if moves_files {
        text.push_str("\n\nthe rename also moved files; that part was not checked before writing");
    }
    Ok(McpToolCallResult::text(text))
}

/// A field renamed together with its accessors (#146): every rename merged into one change per
/// file, checked in one overlay, written only when it compiles unless `force`.
async fn rename_with_accessors(
    remote: SocketAddr,
    workspace_root: &Path,
    file: &Path,
    line: u32,
    character: u32,
    new_name: &str,
    force: bool,
) -> Result<McpToolCallResult> {
    let (merged, renamed) = match crate::rename_accessors::plan(
        remote,
        workspace_root,
        file,
        line,
        character,
        new_name,
    )
    .await
    {
        Ok(plan) => plan,
        Err(e) => return Ok(McpToolCallResult::error(format!("rename refused: {e:#}"))),
    };
    if merged.is_empty() {
        return Ok(McpToolCallResult::error(
            "rename produced no edits".to_string(),
        ));
    }
    let planned: Vec<(std::path::PathBuf, String)> =
        merged.iter().map(|(p, t)| (p.clone(), t.clone())).collect();
    let reports = crate::diagnostics::validate_texts(remote, workspace_root, &planned, &[]).await?;
    let errors: Vec<String> = reports
        .iter()
        .flat_map(|r| {
            r.items
                .iter()
                .filter(|d| d.severity == "error")
                .map(move |d| {
                    format!(
                        "{}{} ({}:{}:{})",
                        d.message.lines().next().unwrap_or(""),
                        d.code
                            .as_deref()
                            .map(|c| format!(" [{c}]"))
                            .unwrap_or_default(),
                        r.file,
                        d.line,
                        d.col
                    )
                })
        })
        .collect();
    if !errors.is_empty() && !force {
        return Ok(McpToolCallResult::error(format!(
            "rename refused: the result does not compile ({} error(s)); nothing was written:\n  {}",
            errors.len(),
            errors.join("\n  ")
        )));
    }
    let touched = crate::refactor::apply_workspace_edit(
        workspace_root,
        &crate::signature::whole_file_edit(&merged),
    )?;
    Ok(McpToolCallResult::text(format!(
        "renamed {}; {} path(s) updated in the checkout:\n{}",
        renamed.join(", "),
        touched.len(),
        touched.join("\n")
    )))
}

async fn handle_check(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let kind = match tool_name {
        "code_check" => crate::verify::VerifyKind::Check,
        "code_lint" => crate::verify::VerifyKind::Lint,
        "code_benchmarks" => crate::verify::VerifyKind::Bench,
        _ => crate::verify::VerifyKind::Test,
    };
    let filter = args
        .get("filter")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let timeout_secs = args
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    // `path` selects a nested project (any file or directory inside it).
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let env = match args.get("env") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_string()))
                    .with_context(|| format!("env `{key}` must be a string, got {value}"))
            })
            .collect::<Result<Vec<_>>>()?,
        Some(other) => anyhow::bail!("env must be an object of strings, got {other}"),
    };
    let fix = args.get("fix").and_then(|v| v.as_bool()).unwrap_or(false);
    if fix
        && matches!(
            kind,
            crate::verify::VerifyKind::Check | crate::verify::VerifyKind::Lint
        )
    {
        anyhow::ensure!(
            env.is_empty(),
            "env is not passed to a `fix` run; run without `fix` to set it"
        );
        let fixed = crate::fixit::check_and_fix(
            remote,
            workspace_root,
            hint.as_deref(),
            kind,
            timeout_secs,
        )
        .await?;
        let text = fixed.render(40);
        return Ok(if fixed.ok() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let report = crate::verify::run_verify_with(
        remote,
        workspace_root,
        hint.as_deref(),
        kind,
        filter.as_deref(),
        timeout_secs,
        &env,
        |_| {},
    )
    .await?;
    let text = report.render(40);
    Ok(if report.ok() {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

/// rust-analyzer writes a prelude item an assist introduces by its full path — an extracted
/// function returns `std::prelude::v1::Result<T, anyhow::Error>` in a file that imports
/// `anyhow::Result` (#97). On the lines the assist wrote, the path is dropped and the result
/// checked in the overlay; the shorter spelling is used only when the
/// analyzer accepts it, and rust-analyzer's own otherwise. Returns the edit to apply and how
/// many paths were shortened.
async fn prefer_names_in_scope(
    remote: SocketAddr,
    root: &Path,
    edit: serde_json::Value,
) -> Result<(serde_json::Value, usize)> {
    const PRELUDE: &str = "std::prelude::v1::";
    let (planned, moves_files) = crate::refactor::planned_texts(root, &edit)?;
    if moves_files {
        return Ok((edit, 0));
    }
    let mut shortened = 0usize;
    let mut shorter = Vec::with_capacity(planned.len());
    for (path, text) in planned {
        // Only lines the assist wrote: a line that was already in the file keeps its spelling,
        // whatever it says.
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        let old_lines: std::collections::HashSet<&str> = before.lines().collect();
        let mut out = String::with_capacity(text.len());
        for line in text.split_inclusive('\n') {
            let body = line.trim_end_matches('\n');
            if body.contains(PRELUDE) && !old_lines.contains(body) {
                shortened += body.matches(PRELUDE).count();
                out.push_str(&line.replace(PRELUDE, ""));
            } else {
                out.push_str(line);
            }
        }
        shorter.push((path, out));
    }
    if shortened == 0 {
        return Ok((edit, 0));
    }
    let reports = crate::diagnostics::validate_texts(remote, root, &shorter, &[]).await?;
    if reports.iter().any(|r| r.errors > 0) {
        return Ok((edit, 0));
    }
    let files: std::collections::BTreeMap<std::path::PathBuf, String> =
        shorter.into_iter().collect();
    Ok((crate::signature::whole_file_edit(&files), shortened))
}

async fn handle_assists(
    remote: SocketAddr,
    workspace_root: &Path,
    tool_name: &str,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let (end_line, end_char) = match (
        args.get("end_line").and_then(|v| v.as_u64()),
        args.get("end_character").and_then(|v| v.as_u64()),
    ) {
        (Some(l), Some(c)) => (l as u32, c as u32),
        _ => (line, character),
    };
    let file_path = resolve_file_path(workspace_root, path_str);
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let mut params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "range": {
            "start": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            "end": { "line": end_line.saturating_sub(1), "character": end_char.saturating_sub(1) }
        }
    });
    if tool_name == "code_assist" {
        let id = args
            .get("id")
            .and_then(|v| v.as_str())
            .context("Missing 'id' argument")?;
        params["id"] = serde_json::json!(id);
        if let Some(subtype) = args.get("subtype").and_then(|v| v.as_u64()) {
            params["subtype"] = serde_json::json!(subtype);
        }
        let edit = match execute_lsp_query(
            remote,
            workspace_root,
            &file_path,
            "prodCode/applyAssist",
            params,
        )
        .await
        {
            Ok(edit) => edit,
            Err(e) => {
                return Ok(McpToolCallResult::error(format!("assist refused: {e:#}")));
            }
        };
        let (edit, respelled) = prefer_names_in_scope(remote, workspace_root, edit).await?;
        let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
        let mut text = format!(
            "applied `{id}`; {} path(s) updated in the checkout:\n{}",
            touched.len(),
            touched.join("\n")
        );
        if respelled > 0 {
            text.push_str(&format!(
                "\n\n{respelled} `std::prelude::v1::` path(s) the assist wrote are spelled as the \
                 name already in scope; the analyzer accepts the shorter spelling"
            ));
        }
        return Ok(McpToolCallResult::text(text));
    }
    let list = execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/assists",
        params,
    )
    .await?;
    let mut out = String::new();
    if let Some(items) = list.as_array() {
        if items.is_empty() {
            out.push_str("no code actions at this position\n");
        }
        for item in items {
            let id = item.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let kind = item.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");
            match item.get("subtype").and_then(|v| v.as_u64()) {
                Some(st) => out.push_str(&format!("{id} (subtype {st}) [{kind}]: {label}\n")),
                None => out.push_str(&format!("{id} [{kind}]: {label}\n")),
            }
        }
    }
    Ok(McpToolCallResult::text(out.trim_end().to_string()))
}

async fn handle_safe_delete(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument")? as u32;
    let file_path = resolve_file_path(workspace_root, path_str);
    // A parameter goes from the declaration and from every call at once, through
    // `change_signature`, which refuses while the body still uses it.
    let text = std::fs::read_to_string(&file_path).unwrap_or_default();
    if let Some((fn_at, name, kept)) = crate::signature::parameter_at(&text, line, character) {
        let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
        // A trait method's parameter goes from the trait, every implementation and every call,
        // by its position (#194).
        if crate::trait_param::owner_of(&text, fn_at).is_some() {
            let (_, open, close) = crate::signature::param_span(&text, fn_at)
                .context("the method has no parameter list")?;
            let (_, declared) = crate::signature::parse_declared(&text[open..close]);
            let index = declared
                .iter()
                .position(|d| d.name == name)
                .context("the parameter is not in the method's list")?;
            let done = crate::trait_param::remove_parameter(
                remote,
                workspace_root,
                &file_path,
                fn_at,
                index,
                true,
                force,
            )
            .await
            .with_context(|| format!("safe delete of the parameter `{name}` refused"))?;
            let text = done.render(6000);
            return Ok(if done.applied && done.diagnostics.is_empty() {
                McpToolCallResult::text(text)
            } else {
                McpToolCallResult::error(text)
            });
        }
        let request = kept
            .iter()
            .map(|k| crate::signature::parse_param(k))
            .collect::<Result<Vec<_>>>()?;
        let (fl, fc) = crate::signature::line_col_at(&text, fn_at);
        let change = crate::signature::change(
            remote,
            workspace_root,
            &file_path,
            fl,
            fc,
            &request,
            true,
            force,
        )
        .await
        .with_context(|| format!("safe delete of the parameter `{name}` refused"))?;
        let text = format!(
            "the parameter `{name}` is removed, with its argument at every call\n\n{}",
            change.render(6000)
        );
        return Ok(if change.diagnostics.is_empty() {
            McpToolCallResult::text(text)
        } else {
            McpToolCallResult::error(text)
        });
    }
    let file_uri = Url::from_file_path(&file_path)
        .map_err(|_| anyhow::anyhow!("Invalid file path for URI: {:?}", file_path))?
        .to_string();
    let params = serde_json::json!({
        "textDocument": { "uri": file_uri },
        "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) }
    });
    let edit = match execute_lsp_query(
        remote,
        workspace_root,
        &file_path,
        "prodCode/safeDelete",
        params,
    )
    .await
    {
        Ok(edit) => edit,
        Err(e) => {
            return Ok(McpToolCallResult::error(format!(
                "safe delete refused: {e:#}"
            )));
        }
    };
    let touched = crate::refactor::apply_workspace_edit(workspace_root, &edit)?;
    // An answer with no edit is not a deletion: saying "deleted" would be a success that did
    // nothing (#138).
    if touched.is_empty() {
        return Ok(McpToolCallResult::error(
            "safe delete produced no edit; nothing was deleted".to_string(),
        ));
    }
    Ok(McpToolCallResult::text(format!(
        "deleted; {} path(s) updated in the checkout:\n{}",
        touched.len(),
        touched.join("\n")
    )))
}

async fn handle_symbols(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let query = args
        .get("query")
        .and_then(|v| v.as_str())
        .context("Missing 'query' argument")?;
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
    let hint = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(|p| resolve_file_path(workspace_root, p));
    let hits =
        workspace_symbol_search(remote, workspace_root, query, hint.as_deref(), limit).await?;
    if hits.is_empty() {
        return Ok(McpToolCallResult::text(format!(
            "No symbols match `{query}`."
        )));
    }
    let mut out = format!("{} symbol(s) matching `{query}`:\n", hits.len());
    for hit in &hits {
        out.push_str(&format!("  {}\n", hit.render(workspace_root)));
    }
    Ok(McpToolCallResult::text(out.trim_end()))
}

async fn handle_migrate_type(
    remote: SocketAddr,
    workspace_root: &Path,
    args: &serde_json::Value,
) -> Result<McpToolCallResult> {
    let path_str = args
        .get("path")
        .and_then(|v| v.as_str())
        .context("Missing 'path' argument (or `symbol`)")?;
    let line = args
        .get("line")
        .and_then(|v| v.as_u64())
        .context("Missing 'line' argument (or `symbol`)")? as u32;
    let character = args
        .get("character")
        .and_then(|v| v.as_u64())
        .context("Missing 'character' argument (or `symbol`)")? as u32;
    let to = args
        .get("to")
        .and_then(|v| v.as_str())
        .context("Missing 'to' argument: the type it should become")?;
    let convert = args
        .get("convert")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let apply = args.get("apply").and_then(|v| v.as_bool()).unwrap_or(false);
    let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
    let file_path = resolve_file_path(workspace_root, path_str);
    let done = crate::type_migration::migrate(
        remote,
        workspace_root,
        &file_path,
        line,
        character,
        to,
        convert,
        apply,
        force,
    )
    .await?;
    let clean = done.sites.is_empty();
    let text = done.render(40);
    Ok(if clean {
        McpToolCallResult::text(text)
    } else {
        McpToolCallResult::error(text)
    })
}

fn resolve_file_path(workspace_root: &Path, path_str: &str) -> std::path::PathBuf {
    let p = std::path::PathBuf::from(path_str);
    if p.is_absolute() {
        p
    } else {
        workspace_root.join(p)
    }
}

/// Helper to connect, initialize, and execute a targeted LSP request against the remote gateway.
pub async fn execute_lsp_query(
    remote: SocketAddr,
    workspace_root: &Path,
    file_path: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    // One long-lived session per checkout for the life of this process (see
    // crate::session::pooled_query): local edits are pushed before the query.
    crate::session::pooled_query(remote, workspace_root, file_path, method, params).await
}

/// What asking the compiler added to a write tool's run.
struct CompileGate {
    text: String,
    passed: bool,
    applied: bool,
}

/// `verify: "compile"`. The tool has built its edit without writing it; the compiler judges it in
/// a shadow of the workspace, and only a result both the analyzer and the compiler accept is
/// written. The overlay check alone does not see an unresolved type (#63), which is precisely
/// what a tool that creates or moves a name can produce.
async fn compile_gate(
    remote: SocketAddr,
    root: &Path,
    files: &[(String, String)],
    analyzer_clean: bool,
    apply: bool,
    force: bool,
) -> Result<CompileGate> {
    if !analyzer_clean && !force {
        return Ok(CompileGate {
            text: "\nthe compiler was not asked: the analyzer already rejects the result\n".into(),
            passed: false,
            applied: false,
        });
    }
    let verdict = crate::compile_check::check(remote, root, files).await?;
    let mut text = verdict.render();
    let mut applied = false;
    if apply {
        if verdict.passed || force {
            let files: std::collections::BTreeMap<std::path::PathBuf, String> = files
                .iter()
                .map(|(p, t)| (std::path::PathBuf::from(p), t.clone()))
                .collect();
            crate::refactor::apply_workspace_edit(
                root,
                &crate::signature::whole_file_edit(&files),
            )?;
            applied = true;
        } else {
            text.push_str(
                "\nnothing was written: the compiler rejects it. Pass `force: true` to write it anyway.\n",
            );
        }
    }
    Ok(CompileGate {
        text,
        passed: verdict.passed,
        applied,
    })
}

/// Tools whose `symbol` is the name of the thing to act on rather than a way of pointing at a
/// position. They are not symbol-addressable: nothing resolves their `symbol` to a
/// path/line/character before the handler runs, because the handler wants the name itself.
#[cfg(test)]
const NAMES_A_SYMBOL: &[&str] = &["code_generate_fixture"];

/// Tools that accept `symbol` in place of `path`/`line`/`character`.
const SYMBOL_ADDRESSABLE: &[&str] = &[
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
];

/// One `workspace/symbol` hit, positioned on the symbol's name (1-based).
#[derive(Debug, Clone)]
pub struct SymbolHit {
    pub path: std::path::PathBuf,
    pub name: String,
    pub kind: &'static str,
    pub container: Option<String>,
    pub line: u32,
    pub col: u32,
}

impl SymbolHit {
    pub fn render(&self, root: &Path) -> String {
        let rel = self.path.strip_prefix(root).unwrap_or(&self.path).display();
        let container = self
            .container
            .as_deref()
            .map(|c| format!("{c}::"))
            .unwrap_or_default();
        format!(
            "[{}] {container}{} — {rel}:{}:{}",
            self.kind, self.name, self.line, self.col
        )
    }
}

fn symbol_kind_name(kind: u64) -> &'static str {
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
}

/// `workspace/symbol` through the pooled session of the project `hint` belongs to (the root
/// when absent). Hits without a range (LSP `WorkspaceSymbol` without resolve) are skipped.
pub async fn workspace_symbol_search(
    remote: SocketAddr,
    root: &Path,
    query: &str,
    hint: Option<&Path>,
    limit: usize,
) -> Result<Vec<SymbolHit>> {
    // The LSP servers (tsc, clangd, pyright) index a project once one of its files is open;
    // the session opens the anchor file before the query, so pick a real source file when the
    // caller gave none or a directory.
    let anchor = match hint {
        Some(h) if h.is_file() => h.to_path_buf(),
        Some(h) => representative_source_file(h).unwrap_or_else(|| h.to_path_buf()),
        None => representative_source_file(root).unwrap_or_else(|| root.to_path_buf()),
    };
    let params = serde_json::json!({ "query": query, "limit": limit.max(1) });
    let mut res =
        execute_lsp_query(remote, root, &anchor, "workspace/symbol", params.clone()).await?;
    if res.as_array().is_none_or(|a| a.is_empty()) {
        // A project that has just been opened may still be loading: one short retry.
        tokio::time::sleep(std::time::Duration::from_millis(800)).await;
        res = execute_lsp_query(remote, root, &anchor, "workspace/symbol", params).await?;
    }
    let mut hits = Vec::new();
    for sym in res.as_array().into_iter().flatten() {
        let Some(name) = sym.get("name").and_then(|n| n.as_str()) else {
            continue;
        };
        let Some(uri) = sym.pointer("/location/uri").and_then(|u| u.as_str()) else {
            continue;
        };
        let Some(path) = Url::parse(uri).ok().and_then(|u| u.to_file_path().ok()) else {
            continue;
        };
        let Some(start) = sym.pointer("/location/range/start") else {
            continue;
        };
        let line = start.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32 + 1;
        let col = start.get("character").and_then(|c| c.as_u64()).unwrap_or(0) as u32 + 1;
        hits.push(SymbolHit {
            path,
            name: name.to_string(),
            kind: symbol_kind_name(sym.get("kind").and_then(|k| k.as_u64()).unwrap_or(0)),
            container: sym
                .get("containerName")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
                .map(str::to_string),
            line,
            col,
        });
        if hits.len() >= limit {
            break;
        }
    }
    Ok(hits)
}

/// Resolves a (possibly qualified) symbol name to one position. Qualifiers (`Type::name`,
/// `pkg.Func`, `Class.method`) are matched against the hit's container, `hint` (a file or
/// directory) prefers hits under it. A tie between different locations is an error listing
/// the candidates.
pub async fn resolve_symbol(
    remote: SocketAddr,
    root: &Path,
    symbol: &str,
    hint: Option<&Path>,
) -> Result<SymbolHit> {
    let parts: Vec<&str> = symbol
        .split(['.', ':', '#', '/'])
        .map(|p| p.trim().trim_end_matches("()"))
        .filter(|p| !p.is_empty())
        .collect();
    let name = parts.last().copied().unwrap_or(symbol);
    let qualifier = parts.len().checked_sub(2).map(|i| parts[i]);
    let hits = workspace_symbol_search(remote, root, name, hint, 200).await?;
    let hint_str = hint.map(|h| h.to_string_lossy().into_owned());
    let mut scored: Vec<(i32, SymbolHit)> = hits
        .into_iter()
        .map(|hit| {
            let mut score = 0;
            if hit.name == name {
                score += 100;
            } else if hit.name.eq_ignore_ascii_case(name) {
                score += 60;
            } else if hit.name.starts_with(name) {
                score += 20;
            }
            if let Some(q) = qualifier {
                match &hit.container {
                    Some(c)
                        if c == q
                            || c.ends_with(&format!("::{q}"))
                            || c.ends_with(&format!(".{q}")) =>
                    {
                        score += 50
                    }
                    Some(c) if c.contains(q) => score += 30,
                    Some(_) => score -= 10,
                    None => {}
                }
            }
            if let Some(h) = &hint_str {
                let p = hit.path.to_string_lossy();
                if *p == **h || p.starts_with(h.as_str()) {
                    score += 30;
                }
            }
            // An index can be stale or (clangd) point the right range at the wrong file:
            // the name must actually be at that position.
            if !identifier_at(&hit.path, hit.line, hit.col, &hit.name) {
                score -= 1000;
            }
            (score, hit)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.path.cmp(&b.1.path))
            .then_with(|| a.1.line.cmp(&b.1.line))
    });
    let Some(best) = scored.first().map(|(s, _)| *s) else {
        anyhow::bail!(
            "no symbol named `{symbol}` in the workspace index (try code_symbols with a shorter name)"
        );
    };
    let ties: Vec<&SymbolHit> = scored
        .iter()
        .filter(|(s, _)| *s == best)
        .map(|(_, h)| h)
        .collect();
    // A re-export (`pub use sync::scan;`) is listed by the index next to the definition it
    // names. It is the same symbol, not a second candidate, and the answer is the definition.
    let definitions: Vec<&SymbolHit> = ties
        .iter()
        .copied()
        .filter(|h| !is_use_declaration(&h.path, h.line))
        .collect();
    let ties = if definitions.is_empty() {
        ties
    } else {
        definitions
    };
    if ties.len() > 1
        && ties
            .iter()
            .any(|h| h.path != ties[0].path || h.line != ties[0].line)
    {
        let mut msg = format!(
            "`{symbol}` is ambiguous ({} candidates); qualify it (Type::name) or pass `path`:\n",
            ties.len()
        );
        for hit in ties.iter().take(10) {
            msg.push_str(&format!("  {}\n", hit.render(root)));
        }
        anyhow::bail!(msg.trim_end().to_string());
    }
    Ok(ties[0].clone())
}

/// The files a workspace edit rewrites, as (path, whole new content). The gateway answers a
/// structural rewrite with `documentChanges`, one whole-file replacement per file, so the
/// caller can diff each against what is on disk.
pub(crate) fn rewritten_files(edit: &serde_json::Value) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for change in edit
        .get("documentChanges")
        .and_then(|c| c.as_array())
        .map(|a| a.as_slice())
        .unwrap_or_default()
    {
        let Some(uri) = change
            .get("textDocument")
            .and_then(|t| t.get("uri"))
            .and_then(|u| u.as_str())
        else {
            continue;
        };
        let Some(new_text) = change
            .get("edits")
            .and_then(|e| e.as_array())
            .and_then(|e| e.first())
            .and_then(|e| e.get("newText"))
            .and_then(|t| t.as_str())
        else {
            continue;
        };
        out.push((crate::remote_fs::uri_to_path(uri), new_text.to_string()));
    }
    out
}

/// A source file of the project at `dir` in its main language (shortest path under `src`
/// first), used to make an LSP server load the project before a workspace-level query.
fn representative_source_file(dir: &Path) -> Option<std::path::PathBuf> {
    let (_, language) = crate::sync::engine_project(dir, dir);
    let exts: &[&str] = match language? {
        "rust" => &["rs"],
        "go" => &["go"],
        "cpp" => &["cpp", "cc", "cxx", "c", "hpp", "h"],
        "python" => &["py"],
        "typescript" => &["ts", "tsx", "mts", "js", "jsx"],
        "swift" => &["swift"],
        _ => return None,
    };
    const SKIP: &[&str] = &[
        "node_modules",
        "target",
        ".git",
        "build",
        "dist",
        ".venv",
        "venv",
        "__pycache__",
        ".build",
        "vendor",
        "Pods",
        "DerivedData",
    ];
    let mut best: Option<(usize, usize, std::path::PathBuf)> = None;
    let mut stack = vec![(dir.to_path_buf(), 0usize)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_dir() {
                if depth < 4 && !SKIP.contains(&name.as_str()) && !name.starts_with('.') {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !exts.contains(&ext) || name.ends_with(".d.ts") {
                continue;
            }
            let rel = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned();
            let outside_src = usize::from(!(rel.starts_with("src/") || rel.starts_with("lib/")));
            let is_test = usize::from(rel.contains("test") || rel.contains("spec"));
            let key = (outside_src + is_test, rel.len());
            if best.as_ref().is_none_or(|(a, b, _)| key < (*a, *b)) {
                best = Some((key.0, key.1, path));
            }
        }
    }
    best.map(|(_, _, p)| p)
}

/// Whether the 1-based `line` of `path` is in a `use` declaration (`use a::b;`, `pub use`,
/// `pub(crate) use`, and any line of one that spans lines, `pub use m::{\n    A,\n    B,\n};`):
/// what the workspace index lists for a re-export, next to the definition it names (#128, #225).
fn is_use_declaration(path: &Path, line: u32) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let lines: Vec<&str> = text.lines().collect();
    let Some(target) = (line as usize).checked_sub(1) else {
        return false;
    };
    // Every `use` runs from its first line to the line with its `;`.
    let mut at = 0;
    while at <= target && at < lines.len() {
        if !starts_use(lines[at]) {
            at += 1;
            continue;
        }
        let end = (at..lines.len())
            .find(|n| lines[*n].contains(';'))
            .unwrap_or(at);
        if (at..=end).contains(&target) {
            return true;
        }
        at = end + 1;
    }
    false
}

/// Does this line start a `use` declaration, with or without a visibility?
fn starts_use(row: &str) -> bool {
    let row = row.trim_start();
    let row = match row.strip_prefix("pub") {
        Some(rest) if rest.starts_with('(') => rest
            .find(')')
            .map_or(rest, |close| &rest[close + 1..])
            .trim_start(),
        Some(rest) if rest.starts_with(char::is_whitespace) => rest.trim_start(),
        _ => row,
    };
    row.starts_with("use ")
}

/// Whether `name` is the identifier at the 1-based line/column of `path` (false when the
/// file cannot be read).
fn identifier_at(path: &Path, line: u32, col: u32, name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Some(row) = text.lines().nth(line.saturating_sub(1) as usize) else {
        return false;
    };
    let start = row
        .char_indices()
        .nth(col.saturating_sub(1) as usize)
        .map(|(i, _)| i)
        .unwrap_or(row.len());
    let bare = name.split(['(', '<']).next().unwrap_or(name);
    row[start..].starts_with(bare)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewritten_files_reads_document_changes_and_skips_the_rest() {
        let edit = serde_json::json!({
            "documentChanges": [
                { "kind": "rename", "oldUri": "file:///w/a.rs", "newUri": "file:///w/b.rs" },
                { "textDocument": { "uri": "file:///w/b.rs", "version": null },
                  "edits": [ { "range": {}, "newText": "fn b() {}\n" } ] },
                { "textDocument": { "uri": "file:///w/c.rs" } }
            ]
        });
        assert_eq!(
            rewritten_files(&edit),
            vec![("/w/b.rs".to_string(), "fn b() {}\n".to_string())]
        );
        assert!(rewritten_files(&serde_json::json!({})).is_empty());
    }

    #[test]
    fn codemod_schema_requires_a_rule_and_offers_apply() {
        let tool = list_tools()
            .into_iter()
            .find(|t| t.name == "code_codemod")
            .expect("code_codemod is listed");
        assert_eq!(tool.input_schema["required"], serde_json::json!(["rule"]));
        assert_eq!(tool.input_schema["properties"]["apply"]["type"], "boolean");
        assert!(
            tool.input_schema["properties"]["rule"]["description"]
                .as_str()
                .unwrap()
                .contains("==>>")
        );
    }

    #[test]
    fn shadow_run_schema_takes_hypotheses_and_argv() {
        let tool = list_tools()
            .into_iter()
            .find(|t| t.name == "code_shadow_run")
            .expect("code_shadow_run is listed");
        let required: Vec<&str> = tool.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["hypotheses", "argv"]);
        let item = &tool.input_schema["properties"]["hypotheses"]["items"];
        assert_eq!(item["required"], serde_json::json!(["name"]));
        assert_eq!(
            item["properties"]["edits"]["items"]["required"],
            serde_json::json!(["path", "new_text"])
        );
        assert_eq!(tool.input_schema["properties"]["apply"]["type"], "boolean");
    }

    #[test]
    fn symbol_addressable_tools_advertise_symbol_and_do_not_require_a_position() {
        for tool in list_tools() {
            let props = tool.input_schema["properties"]
                .as_object()
                .expect("schema has properties");
            let required: Vec<&str> = tool.input_schema["required"]
                .as_array()
                .map(|r| r.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            if SYMBOL_ADDRESSABLE.contains(&tool.name.as_str()) {
                assert!(props.contains_key("symbol"), "{} lacks `symbol`", tool.name);
                assert!(props.contains_key("path"), "{} lacks `path`", tool.name);
                for positional in ["path", "line", "character"] {
                    assert!(
                        !required.contains(&positional),
                        "{} still requires `{positional}`",
                        tool.name
                    );
                }
            } else if !NAMES_A_SYMBOL.contains(&tool.name.as_str()) {
                assert!(
                    !props.contains_key("symbol"),
                    "{} unexpectedly takes `symbol`",
                    tool.name
                );
            }
        }
        let rename = list_tools()
            .into_iter()
            .find(|t| t.name == "code_rename")
            .unwrap();
        let required: Vec<&str> = rename.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(required, vec!["new_name"]);
    }

    #[test]
    fn validate_edits_schema_takes_a_list_of_edits() {
        let tool = list_tools()
            .into_iter()
            .find(|t| t.name == "code_validate_edits")
            .unwrap();
        assert_eq!(tool.input_schema["required"], serde_json::json!(["edits"]));
        assert_eq!(
            tool.input_schema["properties"]["edits"]["items"]["required"],
            serde_json::json!(["path", "new_text"])
        );
    }

    #[test]
    fn outline_schema_offers_locals_toggle() {
        let outline = list_tools()
            .into_iter()
            .find(|t| t.name == "code_outline")
            .unwrap();
        assert!(outline.input_schema["properties"]["include_locals"].is_object());
    }
}
