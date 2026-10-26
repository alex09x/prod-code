/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A qualified lookup verifies the indexed position against the current source (#1420): a
//! stale `run` where the source now says `runner` is not accepted, and the column it answers
//! is in UTF-16 code units, as LSP counts, after supplementary characters too.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn a_stale_prefix_is_not_the_requested_name() {
    let ws = Workspace::new(&[
        ("src/lib.rs", "pub mod m;\n"),
        ("src/m.rs", "pub fn runner() {}\n"),
    ]);
    let root = ws.root();
    let m = root.join("src/m.rs");
    let path = m.clone();
    // The index still knows the function as `run`, at the position `runner` now starts.
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([answers::symbol("run", 12, &path, 1, 8)]),
        _ => serde_json::Value::Null,
    })
    .await;
    let err = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "m::run", None)
        .await
        .expect_err("`runner` is not `run`");
    assert!(format!("{err:#}").contains("no verified symbol"), "{err:#}");
}

#[tokio::test]
async fn a_column_after_supplementary_characters_stays_in_utf16_units() {
    let line = "const FACE: &str = \"😀😀\"; pub fn record() {}";
    let ws = Workspace::new(&[
        ("src/lib.rs", "pub mod m;\n"),
        ("src/m.rs", &format!("{line}\n")),
    ]);
    let root = ws.root();
    let m = root.join("src/m.rs");
    let at = line.find("record").unwrap();
    let utf16_col = line[..at].encode_utf16().count() as u32 + 1;
    let scalar_col = line[..at].chars().count() as u32 + 1;
    assert_eq!(utf16_col, scalar_col + 2, "two supplementary characters");
    let path = m.clone();
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => {
            serde_json::json!([answers::symbol("record", 12, &path, 1, utf16_col)])
        }
        _ => serde_json::Value::Null,
    })
    .await;
    let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "m::record", None)
        .await
        .expect("the declaration is verified at its position");
    assert_eq!((hit.path, hit.line, hit.col), (m, 1, utf16_col));
}
