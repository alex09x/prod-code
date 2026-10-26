/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "rust_method_native/cases.rs"]
mod cases;
#[allow(dead_code)]
#[path = "common/native_rust.rs"]
mod native;
#[path = "rust_method_native/navigation.rs"]
mod navigation;
#[path = "rust_method_native/support.rs"]
mod support;

use native::{Fixture, bridge::Bridge};
use prod_code_mcp::to_method::convert_to_method;
use support::{artifact, position, record_answers, runtime};

const NOMINAL: &str = concat!(
    "struct S(u32);\n",
    "impl S { fn f(s: &S) -> u32 { s.0 + 1 } }\n",
    "fn main() { let a = S(6); let b = S(40); ",
    "println!(\"{} {}\", S::f(&a), S::f(&b)); }\n",
);
const LOCAL_VALUE: &str = concat!(
    "macro_rules! fixed { () => { [u8; { let U = 1; U }] }; }\n",
    "struct S<T>(T);\n",
    "impl S<fixed!()> { fn f<U>(c: &S<fixed!()>) -> u8 { c.0[0] } }\n",
    "fn main() { println!(\"{}\", S::f::<()>(&S([7]))); }\n",
);
const TYPE_CAPTURE: &str = concat!(
    "macro_rules! identity { ($ty:ty) => { ($ty, [u8; { let A = 1; A }]) }; }\n",
    "trait T { fn value(&self)->u8; }\n",
    "struct A {}\n",
    "impl T for A { fn value(&self)->u8 {2} }\n",
    "impl A { fn value(&self)->u8 {1} }\n",
    "struct S<U>(U);\n",
    "impl S<identity!(A)> { fn f<A:T>(c:&S<identity!(A)>)->u8 {c.0.0.value()} }\n",
    "fn main(){println!(\"{}\",S::<(A,[u8;1])>::f::<A>(&S((A {},[7]))));}\n",
);

async fn positive(label: &str, source: &str, declaration: &str, expected: &str, calls: usize) {
    artifact(label, "original.rs", source);
    let before = runtime(label, "original", source);
    assert_eq!(before, expected);
    let fixture = Fixture::new(source, "2021").await;
    let bridge = Bridge::bind().await;
    let addr = bridge.addr;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let client = async {
        // Independent native observations are labelled separately from public traffic.
        support::observe(&fixture, source).await;
        let file = fixture.workspace.root.join("src/lib.rs");
        let (line, col) = position(source, declaration);
        let mut results = Vec::new();
        for apply in [false, true] {
            let result = convert_to_method(
                addr,
                &fixture.workspace.root,
                &file,
                line,
                col,
                apply,
                false,
            )
            .await;
            artifact(
                label,
                &format!("public-{apply}.txt"),
                &format!("{result:#?}\n"),
            );
            // Retain and execute the exact returned bytes even if useful conversion fails.
            if let Ok(plan) = &result {
                artifact(label, &format!("returned-{apply}.rs"), &plan.rewritten[0].1);
                assert_eq!(
                    runtime(label, &format!("returned-{apply}"), &plan.rewritten[0].1),
                    before
                );
            }
            let disk = std::fs::read_to_string(&file).unwrap();
            results.push((apply, result, disk));
        }
        stop.send(true).unwrap();
        results
    };
    let (results, answers) = tokio::join!(client, bridge.run(&fixture, stopped));
    record_answers(label, &answers);
    // Assert only after joined transports finish and all actual replies are retained.
    let mut returned = None;
    for (apply, result, disk) in results {
        let plan = result.expect("supported public convert_to_method must succeed");
        assert_eq!(plan.applied, apply);
        assert_eq!(plan.renamed_uses, 1);
        assert!(plan.diagnostics.is_empty(), "{plan:#?}");
        assert!(plan.unmatched.is_empty(), "{plan:#?}");
        assert_eq!(
            disk,
            if apply {
                plan.rewritten[0].1.as_str()
            } else {
                source
            }
        );
        if let Some(preview) = &returned {
            assert_eq!(preview, &plan.rewritten);
        }
        returned = Some(plan.rewritten.clone());
        if calls > 0 {
            assert_eq!(
                plan.rewritten_calls, calls,
                "exact nominal calls must become useful dot calls: {plan:#?}"
            );
            assert!(plan.rewritten[0].1.contains("a.f()"));
            assert!(plan.rewritten[0].1.contains("b.f()"));
            support::dispatch_identity(&answers, &plan.rewritten[0].1, calls);
        } else {
            assert!(plan.rewritten[0].1.contains("fn f<U>(&self)"));
            assert!(plan.rewritten[0].1.contains("self.0[0]"));
        }
    }
    support::require_methods(&answers, &["textDocument/references"]);
    if calls > 0 {
        support::require_methods(
            &answers,
            &["textDocument/typeDefinition", "textDocument/hover"],
        );
    } else {
        support::require_methods(
            &answers,
            &["rust-analyzer/viewSyntaxTree", "rust-analyzer/expandMacro"],
        );
    }
}

#[tokio::test]
async fn exact_nominal_receiver_converts_direct_calls_with_native_dispatch_identity() {
    positive("nominal", NOMINAL, "f(s:", "7 41\n", 2).await;
}

#[tokio::test]
async fn macro_local_value_keeps_unrelated_method_type_parameter_and_output() {
    positive("local-value", LOCAL_VALUE, "f<U>", "7\n", 0).await;
}

#[tokio::test]
async fn expanded_method_type_capture_refuses_before_writes_including_force() {
    let label = "type-capture";
    artifact(label, "original.rs", TYPE_CAPTURE);
    assert_eq!(runtime(label, "original", TYPE_CAPTURE), "2\n");
    let fixture = Fixture::new(TYPE_CAPTURE, "2021").await;
    let bridge = Bridge::bind().await;
    let addr = bridge.addr;
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let client = async {
        support::observe(&fixture, TYPE_CAPTURE).await;
        let file = fixture.workspace.root.join("src/lib.rs");
        let (line, col) = position(TYPE_CAPTURE, "f<A:T>");
        let mut results = Vec::new();
        for apply in [false, true] {
            for force in [false, true] {
                let result = convert_to_method(
                    addr,
                    &fixture.workspace.root,
                    &file,
                    line,
                    col,
                    apply,
                    force,
                )
                .await;
                artifact(
                    label,
                    &format!("public-{apply}-{force}.txt"),
                    &format!("{result:#?}\n"),
                );
                results.push((result, std::fs::read(&file).unwrap()));
            }
        }
        stop.send(true).unwrap();
        results
    };
    let (results, answers) = tokio::join!(client, bridge.run(&fixture, stopped));
    record_answers(label, &answers);
    for (result, bytes) in results {
        assert_eq!(bytes, TYPE_CAPTURE.as_bytes());
        let error = result
            .expect_err("actual receiver type capture must refuse")
            .to_string();
        assert!(error.contains("shadows the impl self type"), "{error}");
    }
    support::require_methods(
        &answers,
        &["rust-analyzer/viewSyntaxTree", "rust-analyzer/expandMacro"],
    );
}
