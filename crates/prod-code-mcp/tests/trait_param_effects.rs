/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Removing a trait method's parameter keeps the effects of the arguments it drops: a block
//! that assigns, with no call in it, blocks the write like a call does (#1396). Driven end to
//! end against a scripted gateway.

use prod_code_testkit::{Answer, ScriptedGateway, Workspace, answers};
use std::net::SocketAddr;
use std::sync::Arc;

async fn scripted_gateway(answer: Answer) -> SocketAddr {
    ScriptedGateway::start_arc(answer).await.addr()
}

const COUNTED: &str = "pub trait Shape {\n    fn area(&self, scale: u32, unused: u32) -> u32;\n}\n\npub struct Circle(pub u32);\n\nimpl Shape for Circle {\n    fn area(&self, scale: u32, _unused: u32) -> u32 {\n        3 * self.0 * scale\n    }\n}\n\npub fn one(c: &Circle) -> (u32, u32) {\n    let mut counter = 0;\n    let a = c.area(1, { counter += 1; 0 });\n    (a, counter)\n}\n";

#[tokio::test]
async fn a_dropped_argument_that_assigns_blocks_the_write_until_forced() {
    let ws = Workspace::empty();
    let root = ws.root();
    let lib = ws.write("src/lib.rs", COUNTED);
    ws.commit();
    let l = lib.clone();
    let remote = scripted_gateway(Arc::new(move |method, _params| match method {
        "textDocument/definition" => answers::locations(&l, &[(1, 11)]),
        "textDocument/implementation" => answers::locations(&l, &[(8, 8)]),
        "textDocument/references" => answers::locations(&l, &[(8, 8), (15, 15)]),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    }))
    .await;
    let fn_at = COUNTED.match_indices("area").nth(1).unwrap().0;
    let run = |force: bool| {
        let (root, lib) = (root.clone(), lib.clone());
        async move {
            prod_code_mcp::trait_param::remove_parameter(remote, &root, &lib, fn_at, 1, true, force)
                .await
        }
    };

    let blocked = run(false).await.expect("it reports");
    assert_eq!(blocked.parameter, "unused");
    assert_eq!(blocked.calls, 1);
    assert!(
        blocked.blocked.iter().any(|b| b.contains("assigns")),
        "{:?}",
        blocked.blocked
    );
    assert!(!blocked.applied);
    assert_eq!(
        ws.read("src/lib.rs"),
        COUNTED,
        "the counter's increment is not dropped"
    );

    let forced = run(true).await.expect("force writes");
    assert!(forced.applied);
    assert!(ws.read("src/lib.rs").contains("let a = c.area(1);"));
}
