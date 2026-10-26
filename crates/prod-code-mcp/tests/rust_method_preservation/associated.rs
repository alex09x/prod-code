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
async fn associated_move_preserves_literal_bytes_and_nested_self_owner() {
    let source = r####"struct Tax;
struct Order;
impl Order {
    fn f() -> &'static str {
        // Self self tax
        struct Inner;
        impl Inner { fn label() -> &'static str { let _ = Self; "inner" } }
        let _ = Inner::label();
        let _ = Self;
        let _ = b"Self self tax";
        let _ = br##"Self self tax"##;
        let _ = "Self self tax";
        r##"Self self tax"##
    }
}
fn main() { println!("{}", Order::f()); }
"####;
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f() ->", &["f());"])]).await;
    assert_eq!(run(source, "associated original"), "Self self tax\n");
    let plan = move_associated(&ws, remote, false, false).await.unwrap();
    untouched(&ws, source);
    assert_eq!(
        run(&plan.rewritten[0].1, "associated preview"),
        "Self self tax\n"
    );
    assert!(plan.rewritten[0].1.contains("// Self self tax"));
    assert!(plan.rewritten[0].1.contains("let _ = Self; \"inner\""));
    let moved = move_associated(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert_eq!(
        run(&ws.read("src/main.rs"), "associated applied"),
        "Self self tax\n"
    );
}

#[tokio::test]
async fn associated_recursion_and_function_values_follow_the_destination() {
    let source = "struct Tax;\nstruct Order;\nimpl Order {\n    fn f(n: u32) -> u32 {\n        let recur = Self::f;\n        if n == 0 { 1 } else if n % 2 == 0 { recur(n - 1) + 1 } else { Order::f(n - 1) + 1 }\n    }\n}\nfn main() { println!(\"{}\", Order::f(4)); }\n";
    let ws = workspace(source);
    let remote = gateway(&ws, source, &[("f(n:", &["f;", "f(n - 1)", "f(4)"])]).await;
    assert_eq!(run(source, "recursion original"), "5\n");
    let moved = move_associated(&ws, remote, true, false).await.unwrap();
    assert!(moved.applied);
    assert!(moved.unmatched.is_empty());
    assert_eq!(moved.calls, 3);
    assert_eq!(run(&ws.read("src/main.rs"), "recursion applied"), "5\n");
}

#[tokio::test]
async fn cross_module_recursion_uses_the_destination_module_perspective() {
    use prod_code_testkit::{ScriptedGateway, Workspace, answers};
    use serde_json::{Value, json};
    let main =
        "mod origin;\nmod destination;\nfn main() { println!(\"{}\", origin::Old::f(4)); }\n";
    let origin = "pub struct Old;\nimpl Old {\n    pub fn f(n: u32) -> u32 {\n        let recurse = Self::f;\n        let _: Self = Self;\n        if n == 0 { 1 } else if n % 2 == 0 { recurse(n - 1) + 1 } else { Old::f(n - 1) + 1 }\n    }\n}\n";
    let destination = "pub struct Tax;\n";
    assert_eq!(
        run_files(
            main,
            &[("origin.rs", origin), ("destination.rs", destination)],
            "cross-module original"
        ),
        "5\n"
    );
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO),
        ("src/main.rs", main),
        ("src/origin.rs", origin),
        ("src/destination.rs", destination),
    ]);
    let old = ws.path("src/origin.rs");
    let target = ws.path("src/destination.rs");
    let caller = ws.path("src/main.rs");
    let refs = json!([
        answers::locations(
            &old,
            &[position(origin, "f;"), position(origin, "f(n - 1)")]
        )[0]
        .clone(),
        answers::locations(
            &old,
            &[position(origin, "f;"), position(origin, "f(n - 1)")]
        )[1]
        .clone(),
        answers::locations(&caller, &[position(main, "f(4)")])[0].clone()
    ]);
    let remote = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => json!([{"name": "Tax", "kind": 23, "location": {
            "uri": url::Url::from_file_path(&target).unwrap().to_string(),
            "range": {"start": {"line": 0, "character": 11}, "end": {"line": 0, "character": 14}}
        }}]),
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr();
    let (l, c) = position(origin, "f(n:");
    let plan = prod_code_mcp::move_method::move_associated_function(
        remote,
        &ws.root(),
        &old,
        l,
        c,
        "Tax",
        true,
        false,
    )
    .await
    .unwrap();
    assert!(plan.applied);
    assert_eq!(plan.calls, 3);
    let new_main = ws.read("src/main.rs");
    let new_origin = ws.read("src/origin.rs");
    let new_destination = ws.read("src/destination.rs");
    assert!(new_destination.contains("let _: crate::origin::Old = crate::origin::Old;"));
    assert!(new_destination.contains("let recurse = crate::destination::Tax::f;"));
    assert_eq!(
        run_files(
            &new_main,
            &[
                ("origin.rs", &new_origin),
                ("destination.rs", &new_destination)
            ],
            "cross-module applied"
        ),
        "5\n"
    );
}
