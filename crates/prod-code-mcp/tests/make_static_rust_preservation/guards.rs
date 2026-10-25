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
use serde_json::Value;

#[tokio::test]
async fn true_trait_impls_still_refuse_even_when_forced() {
    for header in [
        "impl Trait for S",
        "impl Trait\nfor\nS",
        "impl Trait for/* separator */ S",
    ] {
        let source = format!(
            "struct S;\ntrait Trait {{ fn consume(&self); }}\n{header} {{\n    fn consume(&self) {{}}\n}}\nfn main() {{}}\n"
        );
        let ws = workspace(&source);
        let remote = gateway(serde_json::json!([])).await;
        // Select the implementation, not the trait declaration.
        let (line, col) = position(&source, "consume(&self) {}");
        for apply in [false, true] {
            for force in [false, true] {
                let result = prod_code_mcp::make_static::make_static(
                    remote,
                    &ws.root(),
                    &ws.path("src/main.rs"),
                    line,
                    col,
                    apply,
                    force,
                )
                .await;
                assert!(result.is_err(), "accepted trait impl {header}");
                untouched(&ws, &source);
            }
        }
    }
}

#[tokio::test]
async fn incomplete_references_and_function_values_still_block_force() {
    for (suffix, needle) in [
        ("fn main() { let _f = S::consume; }\n", "consume;"),
        ("fn main() { let _ = 1; }\n", "1;"),
    ] {
        let source = format!("struct S;\nimpl S {{ fn consume(&self) {{}} }}\n{suffix}");
        let ws = workspace(&source);
        let remote = gateway(references(&ws, &source, &[needle])).await;
        let plan = change(&ws, remote, false, true).await.unwrap();
        assert_eq!(plan.unmatched.len(), 1);
        for force in [false, true] {
            assert!(
                change(&ws, remote, true, force)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("not rewritten")
            );
            untouched(&ws, &source);
        }
    }
}

#[tokio::test]
async fn reference_errors_still_refuse_without_writes() {
    let source = "struct S;\nimpl S { fn consume(&self) {} }\nfn main() {}\n";
    let ws = workspace(source);
    let remote = ScriptedGateway::start(|method, _| match method {
        "textDocument/references" => answers::failure("references unavailable"),
        _ => Value::Null,
    })
    .await
    .addr();
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let error = change(&ws, remote, apply, force).await.unwrap_err();
        assert!(format!("{error:#}").contains("cannot find the calls"));
        untouched(&ws, source);
    }
}

#[tokio::test]
async fn effectful_borrowed_receivers_still_block_unforced_apply() {
    for call in ["make().consume()", "S::consume(&make())"] {
        let source = format!(
            "struct S;\nimpl S {{ fn consume(&self) {{}} }}\nfn make() -> S {{ S }}\nfn main() {{ {call}; }}\n"
        );
        let ws = workspace(&source);
        let needle = if call.starts_with("S::") {
            "consume(&make"
        } else {
            "consume();"
        };
        let remote = gateway(references(&ws, &source, &[needle])).await;
        let plan = change(&ws, remote, false, false).await.unwrap();
        assert_eq!(plan.blocked.len(), 1);
        assert!(
            change(&ws, remote, true, false)
                .await
                .unwrap_err()
                .to_string()
                .contains("receiver that does something")
        );
        untouched(&ws, &source);
    }
}
