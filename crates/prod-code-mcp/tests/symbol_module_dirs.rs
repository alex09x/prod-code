/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A directory named like a module file is a module of its own (#1417): `src/main/foo.rs` is
//! `crate::main::foo`, so `crate::main::foo::record` picks it over another `record`.

use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn a_directory_named_main_is_part_of_the_module_path() {
    let ws = Workspace::new(&[
        (
            "Cargo.toml",
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        ("src/lib.rs", "pub mod main;\npub mod other;\n"),
        ("src/main/mod.rs", "pub mod foo;\n"),
        ("src/main/foo.rs", "pub fn record() {}\n"),
        ("src/other.rs", "pub fn record() {}\n"),
    ]);
    let root = ws.root();
    let (foo, other) = (root.join("src/main/foo.rs"), root.join("src/other.rs"));
    let (f, o) = (foo.clone(), other.clone());
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "workspace/symbol" => serde_json::json!([
            answers::symbol("record", 12, &f, 1, 8),
            answers::symbol("record", 12, &o, 1, 8),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    for (symbol, expected) in [
        ("crate::main::foo::record", &foo),
        ("main::foo::record", &foo),
        ("foo::record", &foo),
        ("other::record", &other),
    ] {
        let hit = prod_code_mcp::tools::resolve_symbol(gateway.addr(), &root, symbol, None)
            .await
            .unwrap_or_else(|e| panic!("{symbol}: {e:#}"));
        assert_eq!(&hit.path, expected, "{symbol}");
    }
}
