/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A re-export is the symbol it names, not a candidate of its own: a path hint that only the
//! re-export's file matches does not outrank the definition, and a `;` inside a comment does
//! not end the grouped `use` that holds the name (#1402).

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::Path;

#[tokio::test]
async fn a_hint_at_the_re_exporting_file_still_resolves_to_the_definition() {
    let ws = Workspace::new(&[
        ("src/lib.rs", "pub mod sync;\npub use sync::scan;\n"),
        ("src/sync.rs", "pub fn scan() {}\n"),
        ("src/a.rs", "pub fn load() {}\n"),
        ("src/b.rs", "pub fn load() {}\n"),
    ]);
    let root = ws.root();
    let (lib, sync) = (root.join("src/lib.rs"), root.join("src/sync.rs"));
    let (l, s) = (lib.clone(), sync.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("scan", 12, &l, 2, 15),
            answers::symbol("scan", 12, &s, 1, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "scan",
        Some(Path::new("src/lib.rs")),
    )
    .await
    .expect("the definition is the answer");
    assert_eq!((hit.path, hit.line, hit.col), (sync, 1, 8));

    // Between two real definitions the hint still decides.
    let (a, b) = (root.join("src/a.rs"), root.join("src/b.rs"));
    let (ca, cb) = (a.clone(), b.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("load", 12, &ca, 1, 8),
            answers::symbol("load", 12, &cb, 1, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "load",
        Some(Path::new("src/b.rs")),
    )
    .await
    .expect("the hint picks one definition");
    assert_eq!(hit.path, b);
}

#[tokio::test]
async fn a_semicolon_in_a_comment_does_not_end_a_grouped_use() {
    let ws = Workspace::new(&[
        (
            "src/lib.rs",
            "pub mod m;\npub use m::{ // public API; keep sorted\n    scan,\n};\n",
        ),
        ("src/m.rs", "pub fn scan() {}\n"),
        (
            "src/fields.rs",
            "use crate::m; // one; two\npub struct Holder {\n    pub scan: u8,\n}\n",
        ),
    ]);
    let root = ws.root();
    let (lib, m, fields) = (
        root.join("src/lib.rs"),
        root.join("src/m.rs"),
        root.join("src/fields.rs"),
    );
    let (cl, cm) = (lib.clone(), m.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("scan", 12, &cl, 3, 5),
            answers::symbol("scan", 12, &cm, 1, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "scan", None)
        .await
        .expect("the name in the grouped use is the re-export");
    assert_eq!((hit.path, hit.line, hit.col), (m.clone(), 1, 8));

    // A field below a one-line `use` with a commented `;` is a declaration of its own.
    let (cm, cf) = (m.clone(), fields.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("scan", 12, &cm, 1, 8),
            answers::symbol("scan", 8, &cf, 3, 9),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let err = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "scan", None)
        .await
        .expect_err("the field is a second declaration");
    assert!(format!("{err:#}").contains("ambiguous"), "{err:#}");
}
