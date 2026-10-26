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
use prod_code_testkit::{ScriptedGateway, answers};
use serde_json::{Value, json};

#[tokio::test]
async fn method_type_parameter_does_not_shadow_macro_namespace() {
    let source = "macro_rules! U { () => { u8 }; }\nstruct Tax;\nstruct S<T>(T);\nimpl S<U!()> { fn f<U>(c: &S<U!()>) -> u8 { c.0 } }\nfn main() { println!(\"{}\", S::f::<()>(&S(7))); }\n";
    assert_eq!(run(source, "macro namespace original"), "7\n");
    let mut refused = Vec::new();
    let ws = workspace(source);
    let remote = super::macro_inputs::macro_gateway(&ws, source, "f<U>", "U!", Some("u8")).await;
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
            println!("macro namespace apply={apply} force={force}: {result:?}");
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
            assert!(plan.rewritten[0].1.contains("self.0"));
            assert_eq!(run(&plan.rewritten[0].1, "macro namespace returned"), "7\n");
            if apply {
                assert!(plan.applied);
                assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
            } else {
                untouched(&ws, source);
            }
        }
    }
    assert!(refused.is_empty(), "macro namespace refused: {refused:?}");
}

#[tokio::test]
async fn method_generic_owner_shadowing_refuses_before_any_edits() {
    let source = "trait T { fn value(&self)->u8; }\nstruct S;\nimpl T for S { fn value(&self)->u8 {2} }\nimpl S { fn value(&self)->u8 {1} fn f<S:T>(c:&S)->u8 {c.value()} }\nfn main(){println!(\"{}\",S::f::<S>(&S));}\n";
    assert_eq!(run(source, "generic shadow original"), "2\n");
    let ws = workspace(source);
    let file = ws.path("src/main.rs");
    let binding = position(source, "c:&S");
    let uses = locations(&ws, source, &["c.value()"]);
    let calls = locations(&ws, source, &["f::<S>"]);
    let remote = ScriptedGateway::start(move |method, params| match method {
        "textDocument/references" => {
            let pos = (
                params["position"]["line"].as_u64().unwrap() as u32 + 1,
                params["position"]["character"].as_u64().unwrap() as u32 + 1,
            );
            if pos == binding {
                uses.clone()
            } else {
                calls.clone()
            }
        }
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr();
    for apply in [false, true] {
        for force in [false, true] {
            ws.write("src/main.rs", source);
            let (l, c) = position(source, "f<S:T>");
            let result = prod_code_mcp::to_method::convert_to_method(
                remote,
                &ws.root(),
                &file,
                l,
                c,
                apply,
                force,
            )
            .await;
            println!("generic shadow apply={apply} force={force}: {result:?}");
            let error = result
                .expect_err("shadowed declaration accepted")
                .to_string();
            assert!(error.contains("shadows the impl self type"), "{error}");
            untouched(&ws, source);
        }
    }
}

#[tokio::test]
async fn nested_function_pointer_label_cannot_steal_selected_parameter() {
    let source = "struct Tax(u32);\nstruct Order(u32);\nimpl Order {\n    fn f(&self, helper: fn(tax: &Tax), tax: &Tax) -> u32 { helper(tax); self.0 + tax.0 }\n}\nfn helper(_: &Tax) {}\nfn main() { let o = Order(1); let t = Tax(2); println!(\"{}\", o.f(helper, &t)); }\n";
    assert_eq!(run(source, "function pointer original"), "3\n");
    let ws = workspace(source);
    let file = ws.path("src/main.rs");
    let wrong_binding = position(source, "tax: &Tax)");
    let selected_binding = position(source, "tax: &Tax) ->");
    let wrong_type = (wrong_binding.0, wrong_binding.1 + 6);
    let selected_type = (selected_binding.0, selected_binding.1 + 6);
    let method_pos = position(source, "f(&self");
    let target = position(source, "Tax(u32)");
    let uses = locations(&ws, source, &["tax);", "tax.0"]);
    let calls = locations(&ws, source, &["f(helper"]);
    let remote = ScriptedGateway::start(move |method, params| {
                let pos = (
                    params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1,
                    params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1,
                );
                match method {
                    "textDocument/references" => {
                        println!("references cursor {pos:?}, wrong={wrong_binding:?}, selected={selected_binding:?}");
                        if pos == selected_binding { uses.clone() }
                        else if pos == method_pos { calls.clone() }
                        else { json!([]) }
                    }
                    "textDocument/definition" => {
                        println!("definition cursor {pos:?}, wrong={wrong_type:?}, selected={selected_type:?}");
                        assert_eq!(pos, selected_type, "queried the nested type label");
                        answers::locations(&file, &[target])
                    }
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    _ => Value::Null,
                }
            }).await.addr();
    for apply in [false, true] {
        for force in [false, true] {
            ws.write("src/main.rs", source);
            let moved = move_receiver(&ws, remote, apply, force).await.unwrap();
            println!(
                "function pointer apply={apply} force={force}: {}",
                moved.render(12000)
            );
            if !apply {
                untouched(&ws, source);
            }
            assert_eq!(
                run(&moved.rewritten[0].1, "function pointer returned"),
                "3\n"
            );
            if apply {
                assert!(moved.applied);
                assert_eq!(ws.read("src/main.rs"), moved.rewritten[0].1);
            }
        }
    }
}

#[tokio::test]
async fn ordinary_method_generics_and_self_keep_their_declaration_identity() {
    for (source, declaration) in [
        (
            "struct Tax;\nstruct S(u8);\nimpl S {\n    fn f<U>(c: &S) -> u8 { c.0 }\n}\nfn main() { println!(\"{}\", S::f::<()>(&S(7))); }\n",
            "f<U>",
        ),
        (
            "struct Tax;\nstruct S<T>(T);\nimpl<T: Copy> S<T> {\n    fn f<U>(c: &S<T>) -> T { c.0 }\n}\nfn main() { println!(\"{}\", S::f::<()>(&S(7))); }\n",
            "f<U>",
        ),
        (
            "struct Tax;\nstruct S(u8);\nimpl S {\n    fn f<S>(c: &Self) -> u8 { c.0 }\n}\nfn main() { println!(\"{}\", S::f::<()>(&S(7))); }\n",
            "f<S>",
        ),
        (
            "struct Tax;\nstruct S(u8);\nimpl S {\n    fn f<'a, U, const N: usize>(c: &'a S) -> u8 { c.0 }\n}\nfn main() { println!(\"{}\", S::f::<(), 1>(&S(7))); }\n",
            "f<'a",
        ),
    ] {
        assert_eq!(run(source, "ordinary generics original"), "7\n");
        let ws = workspace(source);
        let remote = gateway(&ws, source, &[("c: &", &["c.0"]), (declaration, &["f::<"])]).await;
        for apply in [false, true] {
            for force in [false, true] {
                ws.write("src/main.rs", source);
                let (l, c) = position(source, declaration);
                let plan = prod_code_mcp::to_method::convert_to_method(
                    remote,
                    &ws.root(),
                    &ws.path("src/main.rs"),
                    l,
                    c,
                    apply,
                    force,
                )
                .await
                .unwrap();
                assert_eq!(plan.renamed_uses, 1);
                if !apply {
                    untouched(&ws, source);
                }
                assert_eq!(
                    run(&plan.rewritten[0].1, "ordinary generics returned"),
                    "7\n"
                );
                if apply {
                    assert!(plan.applied);
                    assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
                }
            }
        }
    }
}

#[tokio::test]
async fn generic_argument_shadowing_refuses_before_any_edits() {
    for owner in ["S<A>", "::This::S<A>"] {
        let source = format!(
            "extern crate self as This;\nstruct Tax;\ntrait T {{ fn value(&self)->u8; }}\nstruct A;\nimpl T for A {{ fn value(&self)->u8 {{2}} }}\nimpl A {{ fn value(&self)->u8 {{1}} }}\nstruct S<U>(U);\nimpl {owner} {{ fn f<A:T>(c:&{owner})->u8 {{c.0.value()}} }}\nfn main(){{println!(\"{{}}\",S::<A>::f::<A>(&S(A)));}}\n"
        );
        assert_eq!(run(&source, "generic argument original"), "2\n");
        let ws = workspace(&source);
        let remote = gateway(
            &ws,
            &source,
            &[("c:&", &["c.0.value()"]), ("f<A:T>", &["f::<A>"])],
        )
        .await;
        for apply in [false, true] {
            for force in [false, true] {
                ws.write("src/main.rs", &source);
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
                println!("generic argument owner={owner} apply={apply} force={force}: {result:?}");
                // Execute any actual unsafe public output before failing the probe.
                if let Ok(plan) = &result {
                    let stdout = run(&plan.rewritten[0].1, "generic argument accepted output");
                    println!("generic argument changed stdout: {stdout:?}");
                }
                let error = result.expect_err("shadowed argument accepted").to_string();
                assert!(error.contains("shadows the impl self type"), "{error}");
                untouched(&ws, &source);
            }
        }
    }
}

#[tokio::test]
async fn absolute_paths_and_qualified_arguments_bypass_local_shadowing() {
    for (owner, generic) in [
        ("::This::S", "This"),
        ("::This::S", "S"),
        ("S<::This::A>", "This"),
        ("S<crate::A>", "A"),
        ("S<types::A>", "A"),
    ] {
        let declarations = if owner.contains('<') {
            "struct A; mod types { pub struct A; } struct S<U>(u8, U);"
        } else {
            "struct S(u8);"
        };
        let value = match owner {
            "S<types::A>" => "S(7, types::A)",
            "S<::This::A>" | "S<crate::A>" => "S(7, A)",
            _ => "S(7)",
        };
        let source = format!(
            "extern crate self as This;\nstruct Tax;\n{declarations}\nimpl {owner} {{ fn f<{generic}>(c:&{owner})->u8 {{c.0}} }}\nfn main(){{println!(\"{{}}\",S::f::<()>(&{value}));}}\n"
        );
        assert_eq!(run(&source, "qualified original"), "7\n");
        let ws = workspace(&source);
        let declaration = format!("f<{generic}>");
        let remote = gateway(
            &ws,
            &source,
            &[("c:&", &["c.0"]), (&declaration, &["f::<()>"])],
        )
        .await;
        for apply in [false, true] {
            for force in [false, true] {
                ws.write("src/main.rs", &source);
                let (l, c) = position(&source, &declaration);
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
                    "qualified owner={owner} generic={generic} apply={apply} force={force}: {result:?}"
                );
                let plan = result.unwrap();
                assert_eq!(plan.renamed_uses, 1);
                assert_eq!(run(&plan.rewritten[0].1, "qualified returned"), "7\n");
                if apply {
                    assert!(plan.applied);
                    assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
                } else {
                    untouched(&ws, &source);
                }
            }
        }
    }
}
