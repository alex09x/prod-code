/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #1508: a constant every call spells alike is inlined only when it is one item, and is bound
//! so it means that item in the callee's module.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

/// The issue's case: `a` and `b` each pass their own `LIMIT`.
const TWO_LIMITS: &str = "mod a {\n    pub const LIMIT: u32 = 1;\n    pub fn run() -> u32 {\n        crate::f(LIMIT)\n    }\n}\nmod b {\n    pub const LIMIT: u32 = 2;\n    pub fn run() -> u32 {\n        crate::f(LIMIT)\n    }\n}\npub const LIMIT: u32 = 100;\npub fn f(n: u32) -> u32 {\n    n\n}\n";

/// One `LIMIT` for both calls, and another, the callee's own, in `mod c`.
const SHADOWED: &str = "pub const LIMIT: u32 = 1;\npub fn run() -> u32 {\n    c::f(LIMIT)\n}\npub fn run2() -> u32 {\n    c::f(LIMIT)\n}\nmod c {\n    const LIMIT: u32 = 100;\n    pub fn f(n: u32) -> u32 {\n        n\n    }\n}\n";

#[tokio::test]
async fn different_constants_of_one_name_are_refused() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", TWO_LIMITS)]);
    let lib = ws.path("src/lib.rs");
    let l = lib.clone();
    let gateway = ScriptedGateway::start_arc(Arc::new(move |method, params| {
        let line = params.pointer("/position/line").and_then(|v| v.as_u64());
        match method {
            "textDocument/references" => answers::locations(&l, &[(4, 16), (10, 16)]),
            // Each call's `LIMIT` is its own module's.
            "textDocument/definition" => match line {
                Some(3) => answers::locations(&l, &[(2, 15)]),
                _ => answers::locations(&l, &[(8, 15)]),
            },
            "textDocument/documentSymbol" => serde_json::json!([]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await;
    let err = prod_code_mcp::inline_parameter::inline_parameter(
        gateway.addr(),
        &ws.root(),
        &lib,
        14,
        10,
        true,
        false,
    )
    .await
    .expect_err("two constants are not one value");
    assert!(format!("{err:#}").contains("it is not one item"), "{err:#}");
    assert_eq!(ws.read("src/lib.rs"), TWO_LIMITS, "nothing was written");
}

#[tokio::test]
async fn a_constant_from_another_module_is_bound_by_its_path() {
    let ws = Workspace::new(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", SHADOWED)]);
    let lib = ws.path("src/lib.rs");
    let l = lib.clone();
    let gateway = ScriptedGateway::start_arc(Arc::new(move |method, _| match method {
        "textDocument/references" => answers::locations(&l, &[(3, 8), (6, 8)]),
        // Both calls name the crate's `LIMIT`; the callee is in `mod c`, which has its own.
        "textDocument/definition" => answers::locations(&l, &[(1, 11)]),
        "textDocument/documentSymbol" => serde_json::json!([{
            "name": "c", "kind": 2,
            "range": { "start": { "line": 7, "character": 0 }, "end": { "line": 12, "character": 1 } },
            "selectionRange": { "start": { "line": 7, "character": 4 }, "end": { "line": 7, "character": 5 } },
            "children": []
        }]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let done = prod_code_mcp::inline_parameter::inline_parameter(
        gateway.addr(),
        &ws.root(),
        &lib,
        10,
        14,
        false,
        false,
    )
    .await
    .expect("one constant, bound by its path");
    assert_eq!(done.value, "crate::LIMIT");
}
