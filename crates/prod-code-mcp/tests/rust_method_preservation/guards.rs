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
use serde_json::json;

#[tokio::test]
async fn failed_reference_queries_refuse_even_force_and_preserve_all_bytes() {
    for (source, declaration, kind) in [
        (
            "struct Tax;\nstruct Order;\nimpl Order {\n    fn f(s: &Order) {}\n}\nfn main() {}\n",
            "s: &Order",
            0,
        ),
        (
            "struct Tax;\nstruct Order;\nimpl Order {\n    fn f(s: &Order) {}\n}\nfn main() {}\n",
            "f(s:",
            0,
        ),
        (
            "struct Tax;\nstruct Order;\nimpl Order {\n    fn f(&self, tax: &Tax) {}\n}\nfn main() {}\n",
            "tax: &Tax",
            1,
        ),
        (
            "struct Tax;\nstruct Order;\nimpl Order {\n    fn f(&self, tax: &Tax) {}\n}\nfn main() {}\n",
            "f(&self",
            1,
        ),
        (
            "struct Tax;\nstruct Order;\nimpl Order {\n    fn f() {}\n}\nfn main() {}\n",
            "f()",
            2,
        ),
    ] {
        let ws = workspace(source);
        let remote = gateway_values(
            &ws,
            source,
            vec![(
                position(source, declaration),
                json!({"error": "reference query failed"}),
            )],
        )
        .await;
        for force in [false, true] {
            ws.write("src/main.rs", source);
            let err = match kind {
                0 => convert(&ws, remote, true, force).await.map(|_| ()),
                1 => move_receiver(&ws, remote, true, force).await.map(|_| ()),
                _ => move_associated(&ws, remote, true, force).await.map(|_| ()),
            }
            .unwrap_err();
            println!("failed references kind={kind} force={force}: {err:#}");
            assert!(err.to_string().contains("cannot find"));
            untouched(&ws, source);
        }
    }
}

#[tokio::test]
async fn effectful_calls_refuse_normally_but_force_constructs_the_rewrite() {
    let source = "struct Tax(u32);\nstruct Order(u32);\nimpl Order {\n    fn f(&self, tax: &Tax) -> u32 { self.0 + tax.0 }\n}\nfn order() -> Order { println!(\"order\"); Order(1) }\nfn tax() -> Tax { println!(\"tax\"); Tax(2) }\nfn main() { println!(\"{}\", order().f(&tax())); }\n";
    let ws = workspace(source);
    let remote = gateway(
        &ws,
        source,
        &[("tax: &Tax", &["tax.0"]), ("f(&self", &["f(&tax())"])],
    )
    .await;
    assert_eq!(run(source, "effectful original"), "order\ntax\n3\n");
    let refused = move_receiver(&ws, remote, true, false).await.unwrap();
    assert!(!refused.applied);
    assert_eq!(refused.blocked.len(), 1);
    untouched(&ws, source);
    let forced = move_receiver(&ws, remote, true, true).await.unwrap();
    assert!(forced.applied);
    assert_eq!(forced.blocked.len(), 1);
    assert_eq!(forced.calls, 1);
    let rewritten = ws.read("src/main.rs");
    assert!(rewritten.contains("tax().f(&order())"), "{rewritten}");
    assert_eq!(run(&rewritten, "effectful forced"), "tax\norder\n3\n");
}

#[tokio::test]
async fn unsupported_references_remain_accounted_and_block_forced_writes() {
    let receiver = "struct Tax;\nstruct Order;\nimpl Order {\n    fn f(&self, tax: &Tax) -> u32 { let _ = tax; 7 }\n}\nfn main() { let value = Order::f; println!(\"{}\", value(&Order, &Tax)); }\n";
    let ws = workspace(receiver);
    let remote = gateway(
        &ws,
        receiver,
        &[("tax: &Tax", &["tax; 7"]), ("f(&self", &["f; println"])],
    )
    .await;
    for force in [false, true] {
        ws.write("src/main.rs", receiver);
        let plan = move_receiver(&ws, remote, true, force).await.unwrap();
        assert!(!plan.applied);
        assert_eq!(plan.unmatched.len(), 1);
        assert!(plan.unmatched[0].contains("used as a value"));
        untouched(&ws, receiver);
        assert_eq!(run(receiver, "receiver value refused"), "7\n");
    }

    let associated = "struct Tax;\nstruct Order;\nimpl Order {\n    fn f() -> u32 { 7 }\n}\nfn main() { println!(\"{}\", <Order>::f()); }\n";
    let ws = workspace(associated);
    let remote = gateway(&ws, associated, &[("f() ->", &["f());"])]).await;
    for force in [false, true] {
        ws.write("src/main.rs", associated);
        let plan = move_associated(&ws, remote, true, force).await.unwrap();
        assert!(!plan.applied);
        assert_eq!(plan.unmatched.len(), 1);
        untouched(&ws, associated);
        assert_eq!(run(associated, "qualified reference refused"), "7\n");
    }
}
