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
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};

#[tokio::test]
async fn analyzer_errors_still_prevent_unforced_writes() {
    let source = "function f(a,b) { return a+b; }\nconsole.log(f(1, 2));\n";
    let ws = Workspace::new(&[("main.js", source)]);
    let root = &ws.root();
    let reference = location(root, "main.js", source, "f(1, 2)");
    let gw = ScriptedGateway::start(move |method, _| match method {
        "textDocument/references" => serde_json::json!([reference]),
        "textDocument/diagnostic" => answers::error_at(1, 1, "test", "intentional analyzer error"),
        _ => serde_json::Value::Null,
    })
    .await;
    let error = change_with(
        gw.addr(),
        root,
        &root.join("main.js"),
        1,
        10,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("the analyzer reports"));
    assert_eq!(
        std::fs::read_to_string(root.join("main.js")).unwrap(),
        source
    );
}

#[tokio::test]
async fn unmatched_references_still_prevent_forced_writes() {
    let source = "function f(a,b) { return a+b; }\nconst callback=f;\nconsole.log(f(1, 2));\n";
    let ws = Workspace::new(&[("main.js", source)]);
    let root = &ws.root();
    let gw = gateway(vec![
        location(root, "main.js", source, "f;"),
        location(root, "main.js", source, "f(1, 2)"),
    ])
    .await;
    let error = change_with(
        gw.addr(),
        root,
        &root.join("main.js"),
        1,
        10,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        true,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("nothing was written"));
    assert_eq!(
        std::fs::read_to_string(root.join("main.js")).unwrap(),
        source
    );
}

#[tokio::test]
async fn ordinary_scalar_reordering_and_dropping_still_succeed() {
    for (args, request, expected) in [
        (
            "1.5e-2, -2",
            vec![Param::Keep("b".into()), Param::Keep("a".into())],
            "[0.015,-2]\n",
        ),
        (
            "'a\\\'b', \"c\"",
            vec![Param::Keep("b".into()), Param::Keep("a".into())],
            "[\"a'b\",\"c\"]\n",
        ),
        (
            "true, false",
            vec![Param::Keep("b".into()), Param::Keep("a".into())],
            "[true,false]\n",
        ),
    ] {
        let source = format!(
            "function f(a,b) {{ return [a,b]; }}\nconsole.log(JSON.stringify(f({args})));\n"
        );
        let ws = Workspace::new(&[("main.js", source.as_str())]);
        let root = &ws.root();
        let gw = gateway(vec![location(
            root,
            "main.js",
            &source,
            &format!("f({args})"),
        )])
        .await;
        change_with(
            gw.addr(),
            root,
            &root.join("main.js"),
            1,
            10,
            &request,
            &Modifiers::default(),
            true,
            false,
        )
        .await
        .unwrap();
        assert_eq!(run(root, "node", &["main.js"]), expected);
    }
    let source = "function f(a,b) { return a; }\nconsole.log(f(7, \"discard\"));\n";
    let ws = Workspace::new(&[("main.js", source)]);
    let root = &ws.root();
    let gw = gateway(vec![location(root, "main.js", source, "f(7,")]).await;
    change_with(
        gw.addr(),
        root,
        &root.join("main.js"),
        1,
        10,
        &[Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap();
    assert_eq!(run(root, "node", &["main.js"]), "7\n");
}
