/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! A Python function that assigns a parameter it would bundle is refused: the write would go
//! to the caller's object, and taken for a keyword argument it was left as a local while its
//! reads became fields (#1509).

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::Arc;

const GROW: &str = "def grow(width: int, height: int) -> int:
    width = width + 1
    return width * height


def area(width: int, height: int) -> int:
    return scale(factor=width) * height


def scale(factor: int) -> int:
    return factor
";

async fn gateway(file: std::path::PathBuf) -> std::net::SocketAddr {
    let f = file.clone();
    ScriptedGateway::start_arc(Arc::new(move |method, params| {
        let line = params.pointer("/position/line").and_then(|v| v.as_u64());
        let ch = params
            .pointer("/position/character")
            .and_then(|v| v.as_u64());
        match (method, line, ch) {
            // `grow`'s `width`: the assignment target, then both reads; `height`: one read.
            ("textDocument/references", Some(0), Some(9)) => {
                answers::locations(&f, &[(2, 5), (2, 13), (3, 12)])
            }
            ("textDocument/references", Some(0), Some(21)) => answers::locations(&f, &[(3, 20)]),
            // `area`'s `width` is read once, inside a keyword argument of another call.
            ("textDocument/references", Some(5), Some(9)) => answers::locations(&f, &[(7, 25)]),
            ("textDocument/references", Some(5), Some(21)) => answers::locations(&f, &[(7, 34)]),
            ("textDocument/references", _, _) => serde_json::json!([]),
            ("textDocument/documentSymbol", _, _) => serde_json::json!([
                answers::document_symbol("grow", 12, 1, 3, 5),
                answers::document_symbol("area", 12, 6, 7, 5),
                answers::document_symbol("scale", 12, 10, 11, 5),
            ]),
            ("textDocument/diagnostic", _, _) => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    }))
    .await
    .addr()
}

#[tokio::test]
async fn assigning_a_bundled_parameter_is_refused_and_keyword_values_are_reads() {
    let ws = Workspace::new(&[
        (
            "pyproject.toml",
            "[project]\nname = \"po-writes\"\nversion = \"0.1.0\"\n",
        ),
        ("app/__init__.py", ""),
        ("app/shapes.py", GROW),
    ]);
    let file = ws.path("app/shapes.py");
    let remote = gateway(file.clone()).await;
    let params = vec!["width".to_string(), "height".to_string()];

    let err = match prod_code_mcp::parameter_object::introduce(
        remote,
        &ws.root(),
        &file,
        1,
        5,
        &params,
        "Size",
        "size",
        false,
        false,
    )
    .await
    {
        Ok(done) => panic!("`grow` was bundled: {}", done.render(4000)),
        Err(err) => format!("{err:#}"),
    };
    assert!(err.contains("assigns its parameter `width`"), "{err}");
    assert_eq!(ws.read("app/shapes.py"), GROW, "nothing was written");

    // `factor=width` passes `width` as a keyword value: the name after `=` is a read.
    let done = prod_code_mcp::parameter_object::introduce(
        remote,
        &ws.root(),
        &file,
        6,
        5,
        &params,
        "Size",
        "size",
        false,
        false,
    )
    .await
    .expect("`area` is bundled");
    let text = done
        .rewritten
        .iter()
        .find(|(p, _)| p.ends_with("shapes.py"))
        .map(|(_, t)| t.clone())
        .expect("shapes.py rewritten");
    assert!(
        text.contains("return scale(factor=size.width) * size.height"),
        "{text}"
    );
}
