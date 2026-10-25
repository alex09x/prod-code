/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Rust supertypes below the first level: the trait graph is followed through the analyzer's
//! `definition` answers, and a lookup that leads nowhere ends its branch quietly.

use prod_code_mcp::supertypes::supertypes;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;

const LIB: &str = "pub trait Base {}\npub trait Mid: Base + Send {}\npub trait Top: Mid + 'static {}\n\n#[derive(Clone)]\npub struct Cache;\n\nimpl Top for Cache {}\nimpl Cache {}\npub struct Plain;\npub trait Odd: Plain + Gone {}\n";
const OTHER: &str = "impl Base for super::Cache {}\n";

fn at(path: &Path, line: u32, character: u32) -> Value {
    json!({ "uri": format!("file://{}", path.display()), "range": {
        "start": { "line": line, "character": character },
        "end": { "line": line, "character": character + 4 } } })
}

async fn gateway(lib: std::path::PathBuf, other: std::path::PathBuf) -> SocketAddr {
    ScriptedGateway::start(move |method, params| {
        let line = params.pointer("/position/line").and_then(Value::as_u64);
        let col = params
            .pointer("/position/character")
            .and_then(Value::as_u64);
        match (method, line, col) {
            // The names under the cursor, and the supertraits named in the headers.
            ("textDocument/definition", Some(2), Some(10)) => at(&lib, 2, 10),
            ("textDocument/definition", Some(2), Some(15)) => at(&lib, 1, 10),
            ("textDocument/definition", Some(1), Some(15)) => at(&lib, 0, 10),
            ("textDocument/definition", Some(5), Some(11)) => at(&lib, 5, 11),
            ("textDocument/definition", Some(7), Some(14)) => at(&lib, 2, 10),
            ("textDocument/definition", Some(0), Some(5)) => at(&lib, 0, 10),
            ("textDocument/definition", Some(10), Some(10)) => at(&lib, 10, 10),
            ("textDocument/definition", Some(10), Some(15)) => at(&lib, 9, 11),
            ("textDocument/definition", Some(10), Some(23)) => at(&lib, 99, 0),
            ("textDocument/implementation", Some(5), Some(11)) => json!([
                at(&lib, 4, 9),
                at(&lib, 7, 14),
                at(&lib, 8, 5),
                at(&lib, 99, 0),
                at(&other, 0, 5)
            ]),
            _ => Value::Null,
        }
    })
    .await
    .addr()
}

fn files() -> Workspace {
    Workspace::new(&[("src/lib.rs", LIB), ("src/other.rs", OTHER)])
}

async fn render(ws: &Workspace, remote: SocketAddr, line: u32, col: u32, depth: usize) -> String {
    supertypes(remote, &ws.root(), &ws.path("src/lib.rs"), line, col, depth)
        .await
        .expect("supertypes run")
        .render(&ws.root())
}

#[tokio::test]
async fn a_trait_is_followed_through_its_supertraits_to_the_asked_depth() {
    let ws = files();
    let remote = gateway(ws.path("src/lib.rs"), ws.path("src/other.rs")).await;
    // A lifetime bound has nothing to look up; a trait the analyzer cannot find ends its branch.
    assert_eq!(
        render(&ws, remote, 3, 11, 3).await,
        "`Top` requires 2 supertrait(s), 4 in all to depth 3:\n  • Mid\n    • Base\n    • Send\n  • 'static"
    );
    assert_eq!(
        render(&ws, remote, 3, 11, 1).await,
        "`Top` requires 2 supertrait(s):\n  • Mid\n  • 'static"
    );
    // Deeper than the limit is read as the limit; at depth 2 the children are leaves.
    assert_eq!(
        render(&ws, remote, 3, 11, 2).await,
        "`Top` requires 2 supertrait(s), 4 in all to depth 2:\n  • Mid\n    • Base\n    • Send\n  • 'static"
    );
}

#[tokio::test]
async fn a_supertrait_that_is_not_a_trait_or_lies_outside_its_file_has_no_children() {
    let ws = files();
    let remote = gateway(ws.path("src/lib.rs"), ws.path("src/other.rs")).await;
    assert_eq!(
        render(&ws, remote, 11, 11, 2).await,
        "`Odd` requires 2 supertrait(s), 2 in all to depth 2:\n  • Plain\n  • Gone"
    );
}

#[tokio::test]
async fn a_type_lists_derives_and_impls_in_any_file_and_follows_each_trait() {
    let ws = files();
    let remote = gateway(ws.path("src/lib.rs"), ws.path("src/other.rs")).await;
    // The inherent impl and an answer past the end of its file are not supertypes.
    assert_eq!(
        render(&ws, remote, 6, 12, 3).await,
        "`Cache` implements 3 trait(s), 7 in all to depth 3:\n  • Base  src/other.rs:1:6\n  • Clone  (derived)  src/lib.rs:5:10\n  • Top  src/lib.rs:8:15\n    • Mid\n      • Base\n      • Send\n    • 'static"
    );
    assert_eq!(
        render(&ws, remote, 6, 12, 1).await,
        "`Cache` implements 3 trait(s):\n  • Base  src/other.rs:1:6\n  • Clone  (derived)  src/lib.rs:5:10\n  • Top  src/lib.rs:8:15"
    );
}

#[tokio::test]
async fn a_failed_implementation_query_is_an_error_and_a_missing_file_is_no_hierarchy() {
    let ws = files();
    let lib = ws.path("src/lib.rs");
    let failing = ScriptedGateway::start({
        let lib = lib.clone();
        move |method, _| match method {
            "textDocument/definition" => at(&lib, 5, 11),
            "textDocument/implementation" => answers::failure("analyzer crashed"),
            _ => Value::Null,
        }
    })
    .await
    .addr();
    let err = supertypes(failing, &ws.root(), &lib, 6, 12, 1)
        .await
        .expect_err("the failure is reported");
    assert!(err.to_string().contains("analyzer crashed"), "{err}");

    let missing = ws.path("src/gone.rs");
    let lost = ScriptedGateway::start(move |method, _| match method {
        "textDocument/definition" => at(&missing, 0, 0),
        _ => Value::Null,
    })
    .await
    .addr();
    assert_eq!(render(&ws, lost, 1, 1, 1).await, "`?` has no supertype.");
}
