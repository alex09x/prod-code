/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::{assert_complete, position, run};
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::{Arc, Mutex};

async fn added_expression(expression: &str, ty: &str, prelude: &str, output: &str) {
    let source =
        format!("{prelude}fn foo() -> u32 {{ 7 }}\nfn main() {{ println!(\"{{}}\", foo()); }}\n");
    let ws = Workspace::new(&[("src/main.rs", &source)]);
    let file = ws.root().join("src/main.rs");
    let original = source.clone();
    let gateway_file = file.clone();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rules = seen.clone();
    let call = position(&source, "foo());");
    // A successful SSR uses exactly the replacement bytes the planner sent. In particular,
    // `"{"` is valid Rust, so an analyzer can successfully return that corrupted literal.
    let gateway = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => answers::locations(&gateway_file, &[call]),
        "prodCode/structuralReplace" => {
            let rule = params["rule"].as_str().unwrap();
            rules.lock().unwrap().push(rule.to_string());
            let replacement = rule.split_once(" ==>> ").unwrap().1;
            let rewritten = original.replace("foo());", &format!("{replacement});"));
            answers::whole_file(&gateway_file, &original, &rewritten)
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let request = [Param::Add {
        name: "_value".into(),
        ty: ty.into(),
        value: expression.into(),
    }];
    let (line, col) = position(&source, "foo()");
    let plan = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        line,
        col,
        &request,
        &Modifiers::default(),
        false,
        false,
    )
    .await
    .unwrap();
    assert_complete(&plan);
    let wanted = format!("foo() ==>> foo({expression})");
    assert_eq!(
        plan.rule, wanted,
        "added expression must remain byte-for-byte"
    );
    assert!(plan.rewritten[0].1.contains(&format!("foo({expression})")));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), source);
    let applied = change_with(
        gateway.addr(),
        &ws.root(),
        &file,
        line,
        col,
        &request,
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap();
    assert_complete(&applied);
    assert!(applied.applied);
    let written = std::fs::read_to_string(&file).unwrap();
    assert_eq!(written, plan.rewritten[0].1);
    assert_eq!(*seen.lock().unwrap(), vec![wanted.clone(), wanted]);
    assert_eq!(run(&written), output);
    if ty == "&str" {
        // Observe the value received by the applied declaration, using an explicit fixture
        // body probe. The planner does not change bodies to consume a newly added parameter.
        let observed = written.replace("{ 7 }", "{ println!(\"{}\", _value); 7 }");
        assert_eq!(
            run(&observed),
            format!("{}\n7\n", &expression[1..expression.len() - 1])
        );
    }
}

#[tokio::test]
async fn successful_ssr_preserves_braces_inside_string_value() {
    added_expression("\"{}\"", "&str", "", "7\n").await;
}

#[tokio::test]
async fn successful_ssr_preserves_struct_and_block_expressions() {
    added_expression(
        "Value { n: 9 }",
        "Value",
        "struct Value { n: u32 }\n",
        "7\n",
    )
    .await;
    added_expression("{ println!(\"block\"); 9 }", "u32", "", "block\n7\n").await;
}

#[tokio::test]
async fn successful_ssr_preserves_ordinary_literals_and_placeholder_like_text() {
    for (expression, ty) in [("42", "u32"), ("\"plain\"", "&str"), ("\"${a0}\"", "&str")] {
        added_expression(expression, ty, "", "7\n").await;
    }
}
