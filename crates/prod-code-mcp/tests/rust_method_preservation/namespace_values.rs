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
use serde_json::{Value, json};
use std::path::Path;

#[path = "../introduce_variable_preservation/analyzer.rs"]
mod analyzer;

pub fn expanded_syntax(file: &Path, expanded: &str) -> Value {
    let text = format!("type __ProdReceiver = {expanded};");
    let mut analyzer = analyzer::Analyzer::start(file, &text);
    analyzer.request(
        "rust-analyzer/viewSyntaxTree",
        json!({"textDocument": {"uri": url::Url::from_file_path(file).unwrap().as_str()}}),
    )
}

#[tokio::test]
async fn expanded_receiver_local_value_does_not_capture_method_type_parameter() {
    let source = "macro_rules! fixed { () => { [u8; { let U = 1; U }] }; }\nstruct S<T>(T);\nimpl S<fixed!()> { fn f<U>(c: &S<fixed!()>) -> u8 { c.0[0] } }\nfn main() { println!(\"{}\", S::f::<()>(&S([7]))); }\n";
    assert_eq!(run(source, "local value original"), "7\n");
    let ws = workspace(source);
    let remote = super::macro_inputs::macro_gateway(
        &ws,
        source,
        "f<U>",
        "fixed!",
        Some("[u8; { let U = 1; U }]"),
    )
    .await;
    let mut refused = Vec::new();
    for apply in [false, true] {
        for force in [false, true] {
            ws.write("src/main.rs", source);
            let (line, col) = position(source, "f<U>");
            let result = prod_code_mcp::to_method::convert_to_method(
                remote,
                &ws.root(),
                &ws.path("src/main.rs"),
                line,
                col,
                apply,
                force,
            )
            .await;
            println!("local value apply={apply} force={force}: {result:?}");
            let plan = match result {
                Ok(plan) => plan,
                Err(error) => {
                    untouched(&ws, source);
                    refused.push(error.to_string());
                    continue;
                }
            };
            assert_eq!(plan.renamed_uses, 1);
            assert!(plan.rewritten[0].1.contains("fn f<U>(&self)"));
            assert!(plan.rewritten[0].1.contains("self.0[0]"));
            assert_eq!(run(&plan.rewritten[0].1, "local value returned"), "7\n");
            if apply {
                assert!(plan.applied);
                assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
            } else {
                untouched(&ws, source);
            }
        }
    }
    assert!(refused.is_empty(), "local value refused: {refused:?}");
}

#[tokio::test]
async fn expanded_receiver_keeps_type_captures_beside_local_values() {
    let source = "macro_rules! identity { ($ty:ty) => { ($ty, [u8; { let A = 1; A }]) }; }\ntrait T { fn value(&self)->u8; }\nstruct A {}\nimpl T for A { fn value(&self)->u8 {2} }\nimpl A { fn value(&self)->u8 {1} }\nstruct S<U>(U);\nimpl S<identity!(A)> { fn f<A:T>(c:&S<identity!(A)>)->u8 {c.0.0.value()} }\nfn main(){println!(\"{}\",S::<(A,[u8;1])>::f::<A>(&S((A {},[7]))));}\n";
    assert_eq!(run(source, "type and local value original"), "2\n");
    let ws = workspace(source);
    let remote = super::macro_inputs::macro_gateway(
        &ws,
        source,
        "f<A:T>",
        "identity!",
        Some("(A, [u8; { let A = 1; A }])"),
    )
    .await;
    for apply in [false, true] {
        for force in [false, true] {
            let (line, col) = position(source, "f<A:T>");
            let result = prod_code_mcp::to_method::convert_to_method(
                remote,
                &ws.root(),
                &ws.path("src/main.rs"),
                line,
                col,
                apply,
                force,
            )
            .await;
            println!("type and local value apply={apply} force={force}: {result:?}");
            let error = result
                .expect_err("captured type beside value accepted")
                .to_string();
            assert!(error.contains("shadows the impl self type"), "{error}");
            untouched(&ws, source);
        }
    }
}

#[tokio::test]
async fn expanded_receiver_requires_namespace_syntax_before_editing() {
    let source = "macro_rules! fixed { () => { [u8; { let U = 1; U }] }; }\nstruct S<T>(T);\nimpl S<fixed!()> { fn f<U>(c: &S<fixed!()>) -> u8 { c.0[0] } }\nfn main() { println!(\"{}\", S::f::<()>(&S([7]))); }\n";
    let ws = workspace(source);
    for syntax in [
        Value::Null,
        json!("{}"),
        json!("{\"kind\":\"SOURCE_FILE\",\"start\":[0,0,0],\"end\":[0,0,0]}"),
    ] {
        let remote = super::macro_inputs::macro_gateway_with_syntax(
            &ws,
            source,
            "f<U>",
            "fixed!",
            Some("[u8; { let U = 1; U }]"),
            syntax,
        )
        .await;
        for apply in [false, true] {
            for force in [false, true] {
                let (line, col) = position(source, "f<U>");
                let result = prod_code_mcp::to_method::convert_to_method(
                    remote,
                    &ws.root(),
                    &ws.path("src/main.rs"),
                    line,
                    col,
                    apply,
                    force,
                )
                .await;
                let error = result
                    .expect_err("unproved value namespace accepted")
                    .to_string();
                println!("unproved namespace apply={apply} force={force}: {error}");
                assert!(error.contains("receiver"), "{error}");
                untouched(&ws, source);
            }
        }
    }
}
