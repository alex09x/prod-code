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
use serde_json::json;

#[tokio::test]
async fn aggregate_raw_owner_and_array_shift_runtime() {
    let mut failures = Vec::new();
    for expression in [
        "let value = r#impl { n: std::mem::size_of::<Self>() }; value.n",
        "let value = r#trait { n: std::mem::size_of::<Self>() }; value.n",
        "let value = r#mod { n: std::mem::size_of::<Self>() }; value.n",
        "struct Inner<T>(std::marker::PhantomData<T>); impl Inner<[u8; 1 << 2]> { fn size() -> usize { std::mem::size_of::<Self>() } } std::mem::size_of::<Self>() + Inner::<[u8; 4]>::size()",
    ] {
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nstruct r#impl {{ n: usize }}\nstruct r#trait {{ n: usize }}\nstruct r#mod {{ n: usize }}\nimpl Order {{\n    fn f() -> usize {{\n        {expression}\n    }}\n}}\nfn main() {{ println!(\"{{}}\", Order::f()); }}\n"
        );
        assert_eq!(run(&source, "aggregate original"), "8\n");
        let ws = workspace(&source);
        let remote = gateway(&ws, &source, &[("f() ->", &["f());"])]).await;
        for force in [false, true] {
            for apply in [false, true] {
                ws.write("src/main.rs", &source);
                let plan = move_associated(&ws, remote, apply, force).await.unwrap();
                println!("aggregate apply={apply} force={force}: {plan:?}");
                assert_eq!(plan.applied, apply);
                if !apply {
                    untouched(&ws, &source);
                }
                let output = run(&plan.rewritten[0].1, "aggregate transformed");
                if output != "8\n" {
                    failures.push(format!(
                        "{expression}: apply={apply} force={force}: {output:?}"
                    ));
                }
                if apply {
                    assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn aggregate_exact_receiver_calls_convert_with_semantic_identity() {
    let source = "struct Tax;\nstruct Counter(u32);\nimpl Counter { fn f(c: &mut Counter, by: u32) -> u32 { c.0 += by; c.0 } }\nfn main() { let mut c = Counter(1); let mut items = [Counter(4)]; println!(\"{} {}\", Counter::f(&mut c, 2), crate::Counter::f(&mut items[0], 3)); }\n";
    assert_eq!(run(source, "exact calls original"), "3 7\n");
    let mut failures = Vec::new();
    let ws = workspace(source);
    let file = ws.path("src/main.rs");
    let uses = locations(&ws, source, &["c.0 +=", "c.0 }"]);
    let calls = locations(&ws, source, &["f(&mut c", "f(&mut items"]);
    let binding = position(source, "c: &mut");
    let c_at = position(source, "c, 2)");
    let items_at = position(source, "items[0]");
    let owner = answers::locations(&file, &[position(source, "Counter(u32)")]);
    let method_location = answers::locations(&file, &[position(source, "f(c:")]);
    let remote = ScriptedGateway::start(move |method, params| {
                let pos = (params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1, params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1);
                match method {
                    "textDocument/references" if pos == binding => uses.clone(),
                    "textDocument/references" => calls.clone(),
                    "textDocument/hover" if pos == c_at => json!({"contents": {"kind": "markdown", "value": "```rust\nlet mut c: Counter\n```"}}),
                    "textDocument/hover" if pos == items_at => json!({"contents": {"kind": "markdown", "value": "```rust\nlet mut items: [Counter; 1]\n```"}}),
                    "textDocument/typeDefinition" => owner.clone(),
                    "textDocument/definition" => { println!("exact dot definition query: {params}"); method_location.clone() },
                    "textDocument/diagnostic" => answers::no_diagnostics(),
                    _ => serde_json::Value::Null,
                }
            }).await.addr();
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", source);
            let plan = convert(&ws, remote, apply, force).await.unwrap();
            println!("exact calls apply={apply} force={force}: {plan:?}");
            if !apply {
                untouched(&ws, source);
            }
            assert_eq!(
                run(&plan.rewritten[0].1, "exact calls transformed"),
                "3 7\n"
            );
            if plan.rewritten_calls != 2 {
                failures.push(format!(
                    "apply={apply} force={force}: {} conversions",
                    plan.rewritten_calls
                ));
            }
            if plan.rewritten_calls == 2 {
                assert!(plan.rewritten[0].1.contains("c.f(2)"));
                assert!(plan.rewritten[0].1.contains("items[0].f(3)"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn aggregate_receiver_move_uses_the_same_owner_identity() {
    for expression in [
        "let value = r#impl { n: std::mem::size_of::<Self>() }; value.n",
        "struct Inner<T>(std::marker::PhantomData<T>); impl Inner<[u8; 1 << 2]> { fn size() -> usize { std::mem::size_of::<Self>() } } std::mem::size_of::<Self>() + Inner::<[u8; 4]>::size()",
    ] {
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nstruct r#impl {{ n: usize }}\nimpl Order {{\n    fn f(&self, tax: &Tax) -> usize {{\n        {expression}\n    }}\n}}\nfn main() {{ let order = Order(1); println!(\"{{}}\", order.f(&Tax)); }}\n"
        );
        assert_eq!(run(&source, "receiver owner original"), "8\n");
        let ws = workspace(&source);
        let remote = gateway(&ws, &source, &[("f(&self", &["f(&Tax)"])]).await;
        for force in [false, true] {
            for apply in [false, true] {
                ws.write("src/main.rs", &source);
                let plan = move_receiver(&ws, remote, apply, force).await.unwrap();
                if !apply {
                    untouched(&ws, &source);
                }
                assert_eq!(
                    run(&plan.rewritten[0].1, "receiver owner transformed"),
                    "8\n"
                );
            }
        }
    }
}

#[tokio::test]
async fn aggregate_raw_literal_cannot_hide_an_implicit_capture() {
    let source = "#[derive(Debug)]\nstruct Tax;\nstruct Order;\nstruct r#impl { n: String }\nimpl Order {\n    fn f(&self, tax: &Tax) -> usize {\n        let value = r#impl { n: format!(\"{tax:?}\") }; value.n.len()\n    }\n}\nfn main() { println!(\"{}\", Order.f(&Tax)); }\n";
    assert_eq!(run(source, "raw capture original"), "3\n");
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f(&self", &["f(&Tax)"])]).await;
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", source);
            let error = move_receiver(&ws, remote, apply, force).await.unwrap_err();
            println!("raw capture apply={apply} force={force}: {error}");
            assert!(error.to_string().contains("capture"), "{error}");
            untouched(&ws, source);
        }
    }
}

#[tokio::test]
async fn aggregate_semantic_wrapper_and_uncertain_identity_stay_qualified() {
    let source = "struct Tax;\nstruct Counter(u32);\nstruct Wrapper(Counter);\nimpl Counter { fn f(c: &mut Counter, by: u32) -> u32 { c.0 += by; c.0 } }\nimpl Wrapper { fn f(&mut self, _: u32) -> u32 { 99 } }\nimpl std::ops::Deref for Wrapper { type Target = Counter; fn deref(&self) -> &Counter { &self.0 } }\nimpl std::ops::DerefMut for Wrapper { fn deref_mut(&mut self) -> &mut Counter { &mut self.0 } }\nfn main() { let mut c = Wrapper(Counter(1)); println!(\"{}\", Counter::f(&mut c, 2)); }\n";
    assert_eq!(run(source, "semantic wrapper original"), "3\n");
    for hover in [Some("Wrapper"), Some("Counter<u64>"), Some("Counter"), None] {
        let ws = workspace(source);
        let uses = locations(&ws, source, &["c.0 +=", "c.0 }"]);
        let calls = locations(&ws, source, &["f(&mut c"]);
        let binding = position(source, "c: &mut");
        let argument = position(source, "c, 2)");
        let owner =
            answers::locations(&ws.path("src/main.rs"), &[position(source, "Counter(u32)")]);
        let other = answers::locations(
            &ws.path("src/main.rs"),
            &[position(source, "Wrapper(Counter)")],
        );
        let remote = ScriptedGateway::start(move |method, params| {
                    let pos = (params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1, params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1);
                    match method {
                        "textDocument/references" if pos == binding => uses.clone(),
                        "textDocument/references" => calls.clone(),
                        "textDocument/hover" => hover.map_or(serde_json::Value::Null, |ty| json!({"contents": {"kind": "markdown", "value": format!("```rust\nlet mut c: {ty}\n```")}})),
                        "textDocument/typeDefinition" if pos == argument => other.clone(),
                        "textDocument/typeDefinition" => owner.clone(),
                        "textDocument/diagnostic" => answers::no_diagnostics(),
                        _ => serde_json::Value::Null,
                    }
                }).await.addr();
        for force in [false, true] {
            for apply in [false, true] {
                ws.write("src/main.rs", source);
                let plan = convert(&ws, remote, apply, force).await.unwrap();
                if !apply {
                    untouched(&ws, source);
                }
                assert_eq!(plan.rewritten_calls, 0);
                assert!(plan.rewritten[0].1.contains("Counter::f(&mut c, 2)"));
                assert_eq!(
                    run(&plan.rewritten[0].1, "semantic wrapper transformed"),
                    "3\n"
                );
            }
        }
    }
}

#[tokio::test]
async fn aggregate_exact_owner_trait_dispatch_needs_overlay_proof() {
    let source = "struct Tax;\nstruct Counter(u32);\ntrait Take { fn f(self, by: u32) -> u32; }\nimpl Take for Counter { fn f(self, _: u32) -> u32 { 99 } }\nimpl Counter { fn f(c: &mut Counter, by: u32) -> u32 { c.0 += by; c.0 } }\nfn main() { let mut c = Counter(1); println!(\"{}\", Counter::f(&mut c, 2)); }\n";
    assert_eq!(run(source, "trait dispatch original"), "3\n");
    let blind_dot = source
        .replace("fn f(c: &mut Counter,", "fn f(&mut self,")
        .replace("c.0 += by; c.0", "self.0 += by; self.0")
        .replace("Counter::f(&mut c, 2)", "c.f(2)");
    assert_eq!(
        run(&blind_dot, "blind exact-owner dot counterexample"),
        "99\n"
    );
    for answer in [0, 1, 2] {
        let ws = workspace(source);
        let file = ws.path("src/main.rs");
        let uses = locations(&ws, source, &["c.0 +=", "c.0 }"]);
        let calls = locations(&ws, source, &["f(&mut c"]);
        let binding = position(source, "c: &mut");
        let owner = answers::locations(&file, &[position(source, "Counter(u32)")]);
        let wrong_method = answers::locations(&file, &[position(source, "f(self, _:")]);
        let remote = ScriptedGateway::start(move |method, params| {
                    let pos = (params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1, params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1);
                    match method {
                        "textDocument/references" if pos == binding => uses.clone(),
                        "textDocument/references" => calls.clone(),
                        "textDocument/hover" => json!({"contents": {"kind": "markdown", "value": "```rust\nlet mut c: Counter\n```"}}),
                        "textDocument/typeDefinition" => owner.clone(),
                        "textDocument/definition" => {
                            println!("trait dot definition answer={answer}: {params}");
                            match answer { 0 => wrong_method.clone(), 1 => serde_json::Value::Null, _ => json!({"error": "lookup failed"}) }
                        },
                        "textDocument/diagnostic" => answers::no_diagnostics(),
                        _ => serde_json::Value::Null,
                    }
                }).await.addr();
        for force in [false, true] {
            for apply in [false, true] {
                ws.write("src/main.rs", source);
                let (line, col) = position(source, "f(c:");
                let plan = prod_code_mcp::to_method::convert_to_method(
                    remote,
                    &ws.root(),
                    &ws.path("src/main.rs"),
                    line,
                    col,
                    apply,
                    force,
                )
                .await
                .unwrap();
                if !apply {
                    untouched(&ws, source);
                }
                assert_eq!(plan.rewritten_calls, 0);
                assert_eq!(run(&plan.rewritten[0].1, "trait dispatch retained"), "3\n");
            }
        }
    }
}
