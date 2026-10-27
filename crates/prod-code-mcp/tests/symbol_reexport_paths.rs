/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A qualified Rust name copied from a `use` line resolves through the `pub use` that makes it
//! (#1582): `tools::render_outline` names `tools/outline.rs`'s `render_outline` because
//! `tools.rs` re-exports it. A name the module does not re-export is still refused.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn a_qualified_name_resolves_through_its_pub_use_path() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"reexports\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod tools;\n"),
        (
            "src/tools.rs",
            "mod outline;\npub use outline::{render_outline, OutlineOptions};\n",
        ),
        (
            "src/tools/outline.rs",
            "pub fn render_outline() {}\npub fn helper() {}\npub struct OutlineOptions;\n",
        ),
    ]);
    let root = ws.root();
    let outline = root.join("src/tools/outline.rs");
    let gateway = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("render_outline", 12, &outline, 1, 8),
            answers::symbol("helper", 12, &outline, 2, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    for symbol in ["tools::render_outline", "reexports::tools::render_outline"] {
        let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, symbol, None)
            .await
            .unwrap_or_else(|err| panic!("{symbol}: {err:#}"));
        assert!(
            hit.path.ends_with("src/tools/outline.rs"),
            "{symbol}: {hit:?}"
        );
        assert_eq!(hit.line, 1, "{symbol}");
    }
    // The defining path still resolves, and a name `tools.rs` does not re-export does not.
    prod_code_mcp::tools::resolve_symbol(
        gateway.addr(),
        &root,
        "tools::outline::render_outline",
        None,
    )
    .await
    .expect("the defining path");
    let err = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, "tools::helper", None)
        .await
        .expect_err("`tools` does not re-export `helper`");
    assert!(
        format!("{err:#}").contains("no symbol named `tools::helper`"),
        "{err:#}"
    );
}
