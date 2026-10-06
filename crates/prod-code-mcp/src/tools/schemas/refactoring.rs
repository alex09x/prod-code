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

pub fn refactoring_tools() -> Vec<McpTool> {
    vec![
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
    ]
}
