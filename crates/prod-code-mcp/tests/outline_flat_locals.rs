/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A flat `documentSymbol` answer has no nesting, so a local is told from a top-level item by
//! whether it starts inside a function's range, character included: `fn f() { let x = 1; }`
//! puts both on one line (#1403). `outline_file` is the renderer the MCP tool and the CLI share.

use prod_code_mcp::tools::{OutlineOptions, outline_file};
use prod_code_testkit::{ScriptedGateway, Workspace};

const SOURCE: &str = "fn f() { let x = 1; } static S: u8 = 0;\n";

fn symbol(name: &str, kind: u32, start: u32, end: u32) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "kind": kind,
        "range": {
            "start": { "line": 0, "character": start },
            "end": { "line": 0, "character": end }
        }
    })
}

#[tokio::test]
async fn a_local_on_the_function_line_is_hidden_and_a_static_after_it_is_not() {
    let ws = Workspace::new(&[("src/lib.rs", SOURCE)]);
    let root = ws.root();
    let lib = root.join("src/lib.rs");
    let gateway = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/documentSymbol" => serde_json::json!([
            symbol("f", 12, 0, 21),
            symbol("x", 13, 9, 19),
            symbol("S", 13, 22, 39),
        ]),
        _ => serde_json::Value::Null,
    })
    .await;
    let outline = |include_locals: bool| {
        let (root, lib) = (root.clone(), lib.clone());
        let addr = gateway.addr();
        async move {
            let options = OutlineOptions::all(usize::MAX, include_locals, "include_locals");
            outline_file(addr, &root, &lib, "src/lib.rs", &options)
                .await
                .expect("the outline renders")
        }
    };

    let text = outline(false).await;
    assert!(text.contains("[Function] f (line 1)"), "{text}");
    assert!(
        !text.contains("[Variable] x"),
        "the local is hidden: {text}"
    );
    assert!(
        text.contains("[Variable] S (line 1)"),
        "a static after the function's range is not its local: {text}"
    );
    assert!(text.contains("1 local variable(s) hidden"), "{text}");

    let text = outline(true).await;
    assert!(text.contains("[Variable] x (line 1)"), "{text}");
    assert!(text.contains("[Variable] S (line 1)"), "{text}");
    assert!(!text.contains("hidden"), "{text}");
}
