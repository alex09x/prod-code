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

pub fn advanced_refactoring_tools() -> Vec<McpTool> {
    vec![
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
    ]
}
