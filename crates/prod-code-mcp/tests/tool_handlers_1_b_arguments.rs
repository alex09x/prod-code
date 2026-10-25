/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The refusals of the refactoring handlers that fire before any analyzer is asked: a missing
//! or malformed argument is reported by name, and the checkout is left as it was. Every case
//! drives `execute_tool` against a gateway that answers nothing, so a handler that reached the
//! analyzer would fail with a different message than the one asserted here.

use prod_code_mcp::tools::execute_tool;
use prod_code_testkit::{ScriptedGateway, Workspace};
use serde_json::{Value, json};

/// Runs each (tool, arguments, exact error) row against a gateway that answers nothing, and
/// reports every row whose error text is not exactly the one expected. `{root}` stands for the
/// checkout's root in the expected text.
async fn refused(ws: &Workspace, rows: &[(&str, Value, &str)]) {
    let gateway = ScriptedGateway::start(|_, _| Value::Null).await;
    let root = ws.root();
    let mut wrong = Vec::new();
    for (tool, args, want) in rows {
        let want = want.replace("{root}", &root.display().to_string());
        match execute_tool(gateway.addr(), &root, tool, args.clone()).await {
            Err(err) if format!("{err:#}") == want => {}
            Err(err) => wrong.push(format!("{tool} {args}:\n  got  {err:#}\n  want {want}")),
            Ok(done) => wrong.push(format!("{tool} {args}: answered {:?}", done.content)),
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

fn fixture() -> Workspace {
    Workspace::new(&[
        ("a.py", "x = 1\n"),
        ("b.rs", "pub fn f() {}\n"),
        ("c.go", "package c\n"),
    ])
}

#[tokio::test]
async fn logic_transforms_name_the_missing_argument() {
    let ws = fixture();
    refused(
        &ws,
        &[
            (
                "code_generify",
                json!({}),
                "Missing 'param' argument: the parameter to make generic",
            ),
            (
                "code_generify",
                json!({ "parameter": "p", "line": 3 }),
                "Missing 'path' or 'symbol' argument",
            ),
            (
                "code_invert_boolean",
                json!({}),
                "Missing 'path' argument (or `symbol`)",
            ),
            (
                "code_invert_boolean",
                json!({ "path": "b.rs" }),
                "Missing 'new_name' argument: the name of the inverted predicate",
            ),
            (
                "code_invert_boolean",
                json!({ "path": "b.rs", "new_name": "g" }),
                "Missing 'line' argument for Rust invert_boolean",
            ),
            (
                "code_invert_boolean",
                json!({ "path": "b.rs", "new_name": "g", "line": 1 }),
                "Missing 'character' argument for Rust invert_boolean",
            ),
        ],
    )
    .await;
}

#[tokio::test]
async fn method_transforms_name_the_missing_argument() {
    let ws = fixture();
    for (tool, rust_name) in [
        ("code_make_static", "make_static"),
        ("code_convert_to_method", "convert_to_method"),
    ] {
        let missing_line = format!("Missing 'line' argument for Rust {rust_name}");
        refused(
            &ws,
            &[
                (tool, json!({}), "Missing 'path' argument (or `symbol`)"),
                (tool, json!({ "path": "b.rs" }), &missing_line),
                (
                    tool,
                    json!({ "path": "a.py" }),
                    "Missing 'method', 'symbol', or line position",
                ),
                (
                    tool,
                    json!({ "path": "a.py", "line": 99 }),
                    "Could not find method at given line",
                ),
                (
                    tool,
                    json!({ "path": "gone.py", "line": 1 }),
                    "No such file or directory (os error 2)",
                ),
            ],
        )
        .await;
    }
    refused(
        &ws,
        &[
            (
                "code_wrap_return",
                json!({ "path": "b.rs" }),
                "Missing 'wrapper' argument: `option`, `result`, `promise`, `pointer`, or custom envelope",
            ),
            (
                "code_wrap_return",
                json!({ "wrapper": "  " }),
                "wrapper cannot be empty: use `option`, `result`, `promise`, `pointer`, or a custom envelope type name",
            ),
            (
                "code_wrap_return",
                json!({ "wrapper": "option" }),
                "Missing 'path' or 'symbol' argument",
            ),
        ],
    )
    .await;
}

#[tokio::test]
async fn extraction_handlers_name_the_missing_argument() {
    let ws = fixture();
    let fields = json!(["x"]);
    refused(
        &ws,
        &[
            ("code_extract_delegate", json!({}), "Missing 'path' argument"),
            (
                "code_extract_delegate",
                json!({ "path": "a.py" }),
                "Specify either `symbol` (or `class`/`type`) or `line` and `character` for the class/struct",
            ),
            (
                "code_extract_delegate",
                json!({ "path": "a.py", "class": "A" }),
                "Missing or empty 'fields' argument",
            ),
            (
                "code_extract_delegate",
                json!({ "path": "a.py", "class": "A", "fields": [] }),
                "Missing or empty 'fields' argument",
            ),
            (
                "code_extract_delegate",
                json!({ "path": "a.py", "type": "A", "fields": fields }),
                "Missing 'name' argument",
            ),
            (
                "code_extract_delegate",
                json!({ "path": "a.py", "symbol": "A", "fields": fields, "name": "H" }),
                "Missing 'field' argument",
            ),
            (
                "code_extract_delegate",
                json!({ "path": "a.py", "symbol": "A", "fields": fields, "name": "H",
                        "field": "h", "verify": "compile" }),
                "`verify: compile` runs `cargo check`, which judges Rust only; a.py is checked by \
                 its language server's diagnostics alone",
            ),
            ("code_extract_trait", json!({}), "Missing 'path' argument"),
            ("code_extract_trait", json!({ "path": "b.rs" }), "Missing 'name' argument"),
            (
                "code_extract_trait",
                json!({ "path": "b.rs", "name": "T" }),
                "Missing 'methods' argument",
            ),
            (
                "code_extract_trait",
                json!({ "path": "b.rs", "name": "T", "methods": ["f"] }),
                "Missing 'line' argument",
            ),
            (
                "code_extract_trait",
                json!({ "path": "b.rs", "name": "T", "methods": ["f"], "line": 1 }),
                "Missing 'character' argument",
            ),
            ("code_extract_function", json!({}), "Missing 'path' argument"),
            ("code_extract_function", json!({ "path": "b.rs" }), "Missing 'name' argument"),
            (
                "code_extract_function",
                json!({ "path": "b.rs", "name": "n" }),
                "Missing 'line' argument",
            ),
            (
                "code_extract_function",
                json!({ "path": "b.rs", "name": "n", "line": 1, "character": 1, "end_line": 1 }),
                "a selection requires both 'end_line' and 'end_character'",
            ),
            ("code_introduce_variable", json!({}), "Missing 'path' argument"),
            (
                "code_introduce_variable",
                json!({ "path": "b.rs" }),
                "Missing 'name' argument",
            ),
            (
                "code_introduce_variable",
                json!({ "path": "b.rs", "name": "v", "line": 1, "character": 1 }),
                "Missing 'end_line' argument",
            ),
        ],
    )
    .await;
}

#[tokio::test]
async fn field_handlers_name_the_missing_argument() {
    let ws = Workspace::new(&[("b.rs", "struct S { n: u32 }\n"), ("a.py", "x = 1\n")]);
    refused(
        &ws,
        &[
            ("code_extract_field", json!({}), "Missing 'path' argument"),
            (
                "code_extract_field",
                json!({ "path": "b.rs" }),
                "Missing selection: provide 'line', 'character', 'end_line', 'end_character', or \
                 'range', or 'expression'",
            ),
            (
                "code_extract_field",
                json!({ "path": "b.rs", "range": "1:1" }),
                "Invalid 'range' format, expected LINE:COL-LINE:COL",
            ),
            (
                "code_extract_field",
                json!({ "path": "b.rs", "range": "1-2" }),
                "Expected LINE:COL",
            ),
            (
                "code_extract_field",
                json!({ "path": "b.rs", "range": "x:1..2:3" }),
                "invalid digit found in string",
            ),
            (
                "code_extract_field",
                json!({ "path": "gone.rs", "expression": "n" }),
                "cannot read {root}/gone.rs: No such file or directory (os error 2)",
            ),
            (
                "code_extract_field",
                json!({ "path": "b.rs", "expression": "absent" }),
                "expression `absent` not found in {root}/b.rs",
            ),
            (
                "code_extract_field",
                json!({ "path": "b.rs", "range": "1:12-1:13" }),
                "Missing 'name' argument: what the new field is called",
            ),
            (
                "code_extract_field",
                json!({ "path": "a.py", "range": "1:1..1:2", "name": "n", "verify": "compile" }),
                "verify: compile is only supported for Rust code_extract_field; no files were written",
            ),
            ("code_encapsulate_field", json!({}), "Missing 'path' argument (or `symbol`)"),
            (
                "code_encapsulate_field",
                json!({ "path": "b.rs" }),
                "Missing 'line' argument for Rust field encapsulation",
            ),
            (
                "code_encapsulate_field",
                json!({ "path": "a.py" }),
                "Missing 'field', 'symbol', or line/character position",
            ),
            (
                "code_encapsulate_field",
                json!({ "path": "a.py", "line": 9 }),
                "Could not find field at given line/character",
            ),
        ],
    )
    .await;
}

#[tokio::test]
async fn schema_rename_and_codemod_refuse_malformed_inputs() {
    let ws = Workspace::new(&[("a.py", "x = 1\n")]);
    refused(
        &ws,
        &[
            (
                "code_schema_rename",
                json!({ "field": "a", "to": "b", "repos": [7] }),
                "repos takes paths, got 7",
            ),
            (
                "code_schema_rename",
                json!({ "field": "a", "to": "b", "repos": "other" }),
                "repos takes a list of paths, got \"other\"",
            ),
        ],
    )
    .await;
    refused(
        &ws,
        &[
            (
                "code_schema_rename",
                json!({ "field": "a", "to": "b", "repos": ["missing-checkout"] }),
                "repository {root}/missing-checkout cannot be read: No such file or directory (os error 2)",
            ),
            (
                "code_schema_rename",
                json!({ "field": "a", "to": "b", "repos": ["."], "path": "a.py" }),
                "`path` and `verify` narrow or check one repository; drop them to rename across `repos`",
            ),
            (
                "code_schema_rename",
                json!({ "field": "a", "to": "b", "repos": ["."], "verify": "compile" }),
                "`path` and `verify` narrow or check one repository; drop them to rename across `repos`",
            ),
        ],
    )
    .await;
}
