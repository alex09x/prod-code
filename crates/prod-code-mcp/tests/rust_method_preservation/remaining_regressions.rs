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

#[tokio::test]
async fn nested_impl_trait_types_preserve_outer_and_independent_self() {
    for (ty, value, read) in [
        (
            "Option<impl Iterator<Item = usize>>",
            "Some(std::iter::once(n))",
            ".unwrap().next().unwrap()",
        ),
        (
            "Result<Option<impl Iterator<Item = usize>>, ()>",
            "Ok(Some(std::iter::once(n)))",
            ".unwrap().unwrap().next().unwrap()",
        ),
        (
            "(impl Iterator<Item = usize>,)",
            "(std::iter::once(n),)",
            ".0.next().unwrap()",
        ),
        (
            "([u8; 1], impl Iterator<Item = usize>)",
            "([0], std::iter::once(n))",
            ".1.next().unwrap()",
        ),
        (
            "impl Iterator<Item = impl std::fmt::Display>",
            "std::iter::once(n)",
            ".next().unwrap()",
        ),
        (
            "&'static impl Send",
            "{ static VALUE: usize = 8; assert_eq!(n, VALUE); &VALUE }",
            "; 8",
        ),
    ] {
        let source = format!(
            "struct Tax;\nstruct Order(u64);\nimpl Order {{\n    fn f() -> {ty} {{\n        struct Inner;\n        let compare: bool = 1 < 2;\n        impl Inner {{ fn size() -> usize {{ std::mem::size_of::<Self>() }} }}\n        assert!(compare);\n        let n = std::mem::size_of::<Self>() + Inner::size();\n        {value}\n    }}\n}}\nfn main() {{ let n = {{ Order::f(){read} }}; println!(\"{{}}\", n); }}\n"
        );
        assert_eq!(run(&source, "nested impl type original"), "8\n");
        let ws = workspace(&source);
        let call = format!("f(){read}");
        let remote = gateway(&ws, &source, &[("f() ->", &[&call])]).await;
        for force in [false, true] {
            ws.write("src/main.rs", &source);
            let plan = move_associated(&ws, remote, false, force).await.unwrap();
            untouched(&ws, &source);
            assert_eq!(run(&plan.rewritten[0].1, "nested impl type preview"), "8\n");
            assert!(
                move_associated(&ws, remote, true, force)
                    .await
                    .unwrap()
                    .applied
            );
            assert_eq!(
                run(&ws.read("src/main.rs"), "nested impl type applied"),
                "8\n"
            );
        }
    }
}

#[tokio::test]
async fn negation_and_never_types_do_not_turn_blocks_into_macros() {
    let source = r##"struct Tax;
struct Order(u64);
impl Order {
    fn f(&self, tax: &Tax, flag: bool) -> &'static str {
        let _ = tax;
        fn diverge() -> ! { let _ = "{self:?}"; loop {} }
        if !flag {
            struct Inner;
            impl Inner { fn size() -> usize { std::mem::size_of::<Self>() } }
            assert_eq!(Inner::size(), 0);
            "{self:?}"
        } else { r#"{self:?}"# }
    }
}
fn main() { let o = Order(2); let t = Tax; println!("{}|{}", o.f(&t, false), o.f(&t, true)); }
"##;
    let expected = "{self:?}|{self:?}\n";
    assert_eq!(run(source, "negation original"), expected);
    let mut refused = 0;
    for (source, expected) in [
        (source.to_string(), expected),
        (
            source
                .replace(
                    "            impl Inner { fn size() -> usize { std::mem::size_of::<Self>() } }",
                    "",
                )
                .replace("            assert_eq!(Inner::size(), 0);", ""),
            expected,
        ),
    ] {
        let ws = workspace(&source);
        let remote = gateway(
            &ws,
            &source,
            &[
                ("tax: &Tax", &["tax;"]),
                ("f(&self", &["f(&t, false)", "f(&t, true)"]),
            ],
        )
        .await;
        for force in [false, true] {
            ws.write("src/main.rs", &source);
            let plan = move_receiver(&ws, remote, false, force).await;
            untouched(&ws, &source);
            println!(
                "negation force={force}: {}",
                plan.as_ref()
                    .map(|_| "planned".to_string())
                    .unwrap_or_else(|e| e.to_string())
            );
            let Ok(plan) = plan else {
                refused += 1;
                let result = move_receiver(&ws, remote, true, force).await;
                println!("negation apply force={force}: {result:?}");
                assert!(result.is_err());
                untouched(&ws, &source);
                continue;
            };
            assert_eq!(run(&plan.rewritten[0].1, "negation preview"), expected);
            assert!(
                move_receiver(&ws, remote, true, force)
                    .await
                    .unwrap()
                    .applied
            );
            assert_eq!(run(&ws.read("src/main.rs"), "negation applied"), expected);
        }
    }
    assert_eq!(
        refused, 0,
        "ordinary negation/never blocks must remain movable"
    );
}

#[tokio::test]
async fn mutable_short_binding_uses_its_token_cursor() {
    let source = "struct Tax(u32);\nstruct Order(u32);\nimpl Order {\n    fn f(&self, mut t: Tax) -> u32 {\n        t.0 += 1; self.0 + t.0\n    }\n}\nfn main() { let o = Order(2); let tax = Tax(3); println!(\"{}\", o.f(tax)); }\n";
    assert_eq!(run(source, "mutable binding original"), "6\n");
    let ws = workspace(source);
    let binding = position(source, "t: Tax");
    let wrong = position(source, "t t: Tax");
    let refs = locations(&ws, source, &["t.0 +=", "t.0\n"]);
    let call_refs = locations(&ws, source, &["f(tax)"]);
    let file = ws.path("src/main.rs");
    let target = position(source, "Tax");
    let remote = prod_code_testkit::ScriptedGateway::start(move |method, params| {
        let pos = (
            params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1,
            params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1,
        );
        match method {
            "textDocument/references" => {
                println!(
                    "binding gateway queried {pos:?}; binding={binding:?}; mut-keyword={wrong:?}"
                );
                if pos == binding {
                    refs.clone()
                } else if pos == position(source, "f(&self") {
                    call_refs.clone()
                } else {
                    serde_json::json!([])
                }
            }
            "textDocument/definition" => prod_code_testkit::answers::locations(&file, &[target]),
            "textDocument/diagnostic" => prod_code_testkit::answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    })
    .await
    .addr();
    for force in [false, true] {
        ws.write("src/main.rs", source);
        let (l, c) = position(source, "f(&self");
        let plan = prod_code_mcp::move_method::move_method(
            remote,
            &ws.root(),
            &ws.path("src/main.rs"),
            l,
            c,
            "t",
            false,
            force,
        )
        .await
        .unwrap();
        untouched(&ws, source);
        assert_eq!(run(&plan.rewritten[0].1, "mutable binding preview"), "6\n");
        let moved = prod_code_mcp::move_method::move_method(
            remote,
            &ws.root(),
            &ws.path("src/main.rs"),
            l,
            c,
            "t",
            true,
            force,
        )
        .await
        .unwrap();
        assert!(moved.applied);
        assert_eq!(
            run(&ws.read("src/main.rs"), "mutable binding applied"),
            "6\n"
        );
    }
}
