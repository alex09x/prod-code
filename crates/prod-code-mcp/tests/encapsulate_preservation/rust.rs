/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::encapsulate_field::encapsulate;
use prod_code_testkit::Workspace;

#[tokio::test]
async fn nested_rhs_compiles_and_preserves_argument_order() {
    let model = "pub struct Config {\n    pub retries: i32,\n}\nimpl Config {\n    pub fn new() -> Self { Self { retries: 1 } }\n}\n";
    let main = "mod model;\nfn pair(a: i32, b: i32) -> i32 { assert_eq!((a,b), (2,3)); a+b }\nfn tick(n: &mut i32) -> i32 { *n += 1; *n }\nfn main() {\n    let mut cfg = model::Config::new();\n    cfg.retries = cfg.retries + 1;\n    let mut n = 1;\n    cfg.retries = pair(tick(&mut n), tick(&mut n)) + cfg.retries;\n    assert_eq!(cfg.retries, 7);\n    println!(\"nested=7 order=2,3\");\n}\n";
    let ws = Workspace::new(&[("model.rs", model), ("main.rs", main)]);
    rust_runtime(&ws);
    let decl = spot(model, "pub retries:", "retries");
    let refs = [
        ("model.rs", spot(model, "Self { retries:", "retries")),
        ("main.rs", spot(main, "retries = cfg", "retries")),
        ("main.rs", spot(main, "retries + 1", "retries")),
        ("main.rs", spot(main, "retries = pair", "retries")),
        ("main.rs", spot(main, "retries;", "retries")),
        ("main.rs", spot(main, "retries, 7", "retries")),
    ];
    let gw = gateway(&ws, "Config", "retries", ("model.rs", decl), &refs).await;
    let preview = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("model.rs"),
        decl.0,
        decl.1,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert!(!preview.applied);
    assert_eq!(ws.read("main.rs"), main);
    let done = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("model.rs"),
        decl.0,
        decl.1,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied);
    rust_runtime(&ws);
    assert!(
        ws.read("main.rs")
            .contains("= ::core::option::Option::Some(cfg.retries() + 1) { () => { cfg.set_retries(prod_code_field_value.unwrap()); }")
    );
}

#[tokio::test]
async fn same_file_module_privacy_preserves_internal_and_descendant_access() {
    let main = "mod inner {\n    pub struct Config {\n        pub retries: i32,\n    }\n    impl Config {\n        pub fn new() -> Self { Self { retries: 1 } }\n        pub fn direct(&self) -> i32 { self.retries }\n    }\n    pub mod child {\n        pub fn direct(c: &super::Config) -> i32 { c.retries }\n    }\n}\nmod sibling {\n    pub fn read(c: &super::inner::Config) -> i32 { c.retries }\n}\nfn main() {\n    let mut cfg = inner::Config::new();\n    cfg.retries = cfg.retries + 1;\n    assert_eq!(cfg.retries, 2);\n    assert_eq!(cfg.direct(), 2);\n    assert_eq!(inner::child::direct(&cfg), 2);\n    assert_eq!(sibling::read(&cfg), 2);\n    println!(\"privacy=2 internal=2 descendant=2 sibling=2\");\n}\n";
    let ws = Workspace::new(&[("main.rs", main)]);
    rust_runtime(&ws);
    let decl = spot(main, "pub retries:", "retries");
    let refs = [
        "Self { retries:",
        "self.retries }",
        "c.retries }\n    }",
        "c.retries }\n}",
        "retries = cfg",
        "retries + 1",
        "retries, 2",
    ]
    .map(|anchor| ("main.rs", spot(main, anchor, "retries")));
    let gw = gateway(&ws, "Config", "retries", ("main.rs", decl), &refs).await;
    let done = encapsulate(
        gw.addr(),
        &ws.root(),
        &ws.path("main.rs"),
        decl.0,
        decl.1,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(done.applied);
    rust_runtime(&ws);
    assert_eq!((done.reads, done.writes, done.left_in_file), (3, 1, 3));
}
