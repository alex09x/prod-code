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
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::net::SocketAddr;

pub async fn macro_gateway(
    ws: &Workspace,
    source: &str,
    declaration: &str,
    invocation: &str,
    expansion: Option<&str>,
) -> SocketAddr {
    let syntax = expansion.map_or(Value::Null, |expanded| {
        super::namespace_values::expanded_syntax(&ws.path("src/main.rs"), expanded)
    });
    macro_gateway_with_syntax(ws, source, declaration, invocation, expansion, syntax).await
}

pub async fn macro_gateway_with_syntax(
    ws: &Workspace,
    source: &str,
    declaration: &str,
    invocation: &str,
    expansion: Option<&str>,
    syntax: Value,
) -> SocketAddr {
    let binding = position(source, "c:");
    let method = position(source, declaration);
    let uses = locations(ws, source, &["c.0"]);
    let calls = locations(ws, source, &["f::<"]);
    // The impl has an identical invocation; query the one in the parameter's scope.
    let parameter = source.find("c:").unwrap();
    let at = parameter + source[parameter..].find(invocation).unwrap();
    let macro_pos = prod_code_mcp::signature::position_at(source, at).unwrap();
    let expansion = expansion.map(str::to_owned);
    let overlay = expansion
        .as_ref()
        .map(|text| format!("type __ProdReceiver = {text};"));
    let opened = std::sync::Mutex::new(String::new());
    let uri = url::Url::from_file_path(ws.path("src/main.rs"))
        .unwrap()
        .to_string();
    ScriptedGateway::start(move |name, params| {
        let pos = (
            params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1,
            params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1,
        );
        match name {
            "textDocument/didOpen" => {
                *opened.lock().unwrap() =
                    params["textDocument"]["text"].as_str().unwrap().to_owned();
                Value::Null
            }
            "rust-analyzer/viewSyntaxTree" => {
                assert_eq!(opened.lock().unwrap().as_str(), overlay.as_deref().unwrap());
                assert_eq!(params["textDocument"]["uri"], uri);
                syntax.clone()
            }
            "rust-analyzer/expandMacro" => {
                assert_eq!(pos, macro_pos, "expansion must use the parameter's scope");
                println!("macro expansion cursor {pos:?}: {expansion:?}");
                expansion.as_ref().map_or(
                    Value::Null,
                    |text| json!({"name": "type macro", "expansion": text}),
                )
            }
            "textDocument/references" if pos == binding => uses.clone(),
            "textDocument/references" if pos == method => calls.clone(),
            "textDocument/references" => json!([]),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => Value::Null,
        }
    })
    .await
    .addr()
}

#[tokio::test]
async fn ignored_macro_inputs_preserve_supported_conversion() {
    let source = "macro_rules! fixed { ($ignored:ident) => { u8 }; }\nstruct S<T>(T);\nimpl S<fixed!(U)> { fn f<U>(c: &S<fixed!(U)>) -> u8 { c.0 } }\nfn main() { println!(\"{}\", S::f::<()>(&S(7))); }\n";
    assert_eq!(run(source, "ignored input original"), "7\n");
    let mut refused = Vec::new();
    let ws = workspace(source);
    let remote = macro_gateway(&ws, source, "f<U>", "fixed!", Some("u8")).await;
    for apply in [false, true] {
        for force in [false, true] {
            ws.write("src/main.rs", source);
            let (l, c) = position(source, "f<U>");
            let result = prod_code_mcp::to_method::convert_to_method(
                remote,
                &ws.root(),
                &ws.path("src/main.rs"),
                l,
                c,
                apply,
                force,
            )
            .await;
            println!("ignored macro input apply={apply} force={force}: {result:?}");
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
            assert_eq!(run(&plan.rewritten[0].1, "ignored input returned"), "7\n");
            if apply {
                assert!(plan.applied);
                assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
            } else {
                untouched(&ws, source);
            }
        }
    }
    assert!(
        refused.is_empty(),
        "ignored macro input refused: {refused:?}"
    );
}

#[tokio::test]
async fn macro_expansion_resolving_method_generic_refuses_before_edits() {
    for invocation in ["identity!(A)", "crate::identity!(A)", "identity!()"] {
        let source = format!(
            "#[macro_export]\nmacro_rules! identity {{ ($ty:ty) => {{ $ty }}; () => {{ A }}; }}\ntrait T {{ fn value(&self)->u8; }}\nstruct A;\nimpl T for A {{ fn value(&self)->u8 {{2}} }}\nimpl A {{ fn value(&self)->u8 {{1}} }}\nstruct S<U>(U);\nimpl S<{invocation}> {{ fn f<A:T>(c:&S<{invocation}>)->u8 {{c.0.value()}} }}\nfn main(){{println!(\"{{}}\",S::<A>::f::<A>(&S(A)));}}\n"
        );
        assert_eq!(run(&source, "resolved macro original"), "2\n");
        for expansion in [Some("A"), None] {
            let ws = workspace(&source);
            let remote = macro_gateway(&ws, &source, "f<A:T>", "identity!", expansion).await;
            for apply in [false, true] {
                for force in [false, true] {
                    let (l, c) = position(&source, "f<A:T>");
                    let result = prod_code_mcp::to_method::convert_to_method(
                        remote,
                        &ws.root(),
                        &ws.path("src/main.rs"),
                        l,
                        c,
                        apply,
                        force,
                    )
                    .await;
                    println!(
                        "resolved macro expansion={expansion:?} apply={apply} force={force}: {result:?}"
                    );
                    let error = result
                        .expect_err("unproved macro identity accepted")
                        .to_string();
                    assert!(
                        error.contains(if expansion.is_some() {
                            "shadows the impl self type"
                        } else {
                            "macro expansion"
                        }),
                        "{error}"
                    );
                    untouched(&ws, &source);
                }
            }
        }
    }
}
