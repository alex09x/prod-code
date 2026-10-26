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

async fn check_calls(expression: &str, needles: &[&str], expected: &str) {
    let source = format!(
        "struct Counter(u32);\nimpl Counter {{ fn f(c: &Counter, by: u32) -> u32 {{ c.0 + by }} }}\nfn main() {{ let c = Counter(1); let result = {expression}; println!(\"{{}}\", result); }}\n"
    );
    assert_eq!(run(&source, "call trivia original"), expected);
    let ws = workspace(&source);
    let file = ws.path("src/main.rs");
    let uses = locations(&ws, &source, &["c.0 +"]);
    let calls = locations(&ws, &source, needles);
    let binding = position(&source, "c: &Counter");
    let owner = answers::locations(&file, &[position(&source, "Counter(u32)")]);
    let method_location = answers::locations(&file, &[position(&source, "f(c:")]);
    let remote = ScriptedGateway::start(move |method, params| {
        let pos = (
            params["position"]["line"].as_u64().unwrap_or(0) as u32 + 1,
            params["position"]["character"].as_u64().unwrap_or(0) as u32 + 1,
        );
        match method {
            "textDocument/references" if pos == binding => uses.clone(),
            "textDocument/references" => calls.clone(),
            "textDocument/hover" => {
                json!({"contents": {"kind": "markdown", "value": "```rust\nlet c: Counter\n```"}})
            }
            "textDocument/typeDefinition" => owner.clone(),
            "textDocument/definition" => method_location.clone(),
            "textDocument/diagnostic" => answers::no_diagnostics(),
            _ => serde_json::Value::Null,
        }
    })
    .await
    .addr();
    for force in [false, true] {
        for apply in [false, true] {
            ws.write("src/main.rs", &source);
            let plan = convert(&ws, remote, apply, force).await.unwrap();
            println!("call trivia apply={apply} force={force}: {plan:?}");
            assert_eq!(plan.rewritten_calls, 1);
            assert_eq!(plan.applied, apply);
            if apply {
                assert_eq!(ws.read("src/main.rs"), plan.rewritten[0].1);
            } else {
                untouched(&ws, &source);
            }
            assert_eq!(
                run(&plan.rewritten[0].1, "call trivia transformed"),
                expected
            );
        }
    }
}

#[tokio::test]
async fn line_comment_argument_retains_its_newline() {
    check_calls("Counter::f(&c, 2 // note\n)", &["f(&c, 2"], "3\n").await;
    check_calls(
        "Counter::f(&c, // before value, keep this comment\n 2 // final value\n)",
        &["f(&c, // before"],
        "3\n",
    )
    .await;
}

#[tokio::test]
async fn nested_exact_owner_calls_do_not_overlap() {
    check_calls(
        "Counter::f(&c, Counter::f(&c, 2))",
        &["f(&c, Counter", "f(&c, 2"],
        "4\n",
    )
    .await;
    check_calls(
        "Counter::f(&c, Counter::f(&c, 2))",
        &["f(&c, 2", "f(&c, Counter"],
        "4\n",
    )
    .await;
}
