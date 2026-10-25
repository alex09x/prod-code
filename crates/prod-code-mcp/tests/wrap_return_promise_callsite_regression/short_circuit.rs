/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::postfix::run_javascript;
use super::support::*;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn method_gateway(ws: &Workspace, source: &str) -> (ScriptedGateway, Arc<AtomicUsize>) {
    let path = ws.root().join("selected.js");
    let at = source.rfind("retry()").unwrap();
    let line = source[..at].bytes().filter(|b| *b == b'\n').count() as u32 + 1;
    let col = source[..at].rsplit('\n').next().unwrap().len() as u32 + 1;
    let diagnostics = Arc::new(AtomicUsize::new(0));
    let count = diagnostics.clone();
    let gw = ScriptedGateway::start(move |method, _| match method {
        "workspace/symbol" => json!([answers::symbol("retry", 6, &path, 2, 5)]),
        "textDocument/references" => answers::locations(&path, &[(line, col)]),
        "textDocument/diagnostic" => {
            count.fetch_add(1, Ordering::SeqCst);
            answers::no_diagnostics()
        }
        _ => Value::Null,
    })
    .await;
    (gw, diagnostics)
}

#[tokio::test]
async fn nonnesting_block_comments_cannot_hide_executable_callers() {
    for gap in [" ", "\n    "] {
        for expression in ["retry()[mark()]", "await retry()[mark()]"] {
            let source = format!(
                "export function retry() {{ events.push(\"selected\"); return [7]; }}\nconst events = [];\nfunction mark() {{ events.push(\"index\"); return 0; }}\nexport async function run() {{\n    /* /* */{gap}return {expression};\n}}\nconsole.log(JSON.stringify([await run(), events]));\n"
            );
            let before = run_javascript(&source);
            let ws = workspace(&source, "js");
            let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
            let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
            eprintln!(
                "comment {gap:?} {expression}: propagated={} unmatched={:?}; {}",
                plan.propagated,
                plan.unmatched,
                planned(&plan, "js")
            );
            assert_eq!(ws.read("selected.js"), source);
            tool(gw.addr(), &ws, "js", true, false).await.unwrap();
            let after = ws.read("selected.js");
            let runtime = run_javascript(&after);
            if let Some(before) = before {
                assert_eq!(before.trim(), "[7,[\"selected\",\"index\"]]");
                assert_eq!(runtime.as_deref(), Some(before.as_str()));
                eprintln!("RUNTIME nonnesting comment value/order verified: {before}");
            }
            assert_eq!(plan.propagated, 1, "{plan:?}");
            assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
            let expected = expression.replace("retry()", "(await retry())");
            assert!(
                after.contains(&format!("/* /* */{gap}return {expected};")),
                "{after}"
            );
            validated(&diagnostics);
        }
    }
}

fn method_source(expression: &str) -> String {
    format!(
        "export class Worker {{\n    retry() {{ events.push(\"selected\"); return [7]; }}\n}}\nconst events = [];\nfunction mark() {{ events.push(\"index\"); return 0; }}\nexport async function run(object) {{\n    return {expression};\n}}\nconsole.log(JSON.stringify([await run(new Worker()), await run(null), events]));\n"
    )
}

#[tokio::test]
async fn optional_receivers_with_postfix_remain_unmatched_and_cannot_write() {
    for expression in [
        "await object?.retry()[mark()]",
        "object /* receiver */ ?. /* member */ retry()[mark()]",
    ] {
        let source = method_source(expression);
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = method_gateway(&ws, &source).await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        eprintln!(
            "optional {expression}: propagated={} unmatched={:?}; {}",
            plan.propagated,
            plan.unmatched,
            planned(&plan, "js")
        );
        assert_eq!(ws.read("selected.js"), source);
        for force in [false, true] {
            let write = tool(gw.addr(), &ws, "js", true, force).await;
            eprintln!(
                "optional apply: refused={}, unchanged={}",
                write.is_err(),
                ws.read("selected.js") == source
            );
            assert!(
                write.is_err(),
                "optional receiver short-circuit must not be terminated"
            );
            assert_eq!(ws.read("selected.js"), source);
            assert!(wrap(gw.addr(), &ws, "js", true, force).await.is_err());
            assert_eq!(ws.read("selected.js"), source);
        }
        assert_eq!(plan.propagated, 0);
        assert!(
            plan.unmatched
                .iter()
                .any(|s| s.contains("optional") && s.contains("selected.js:")),
            "{plan:?}"
        );
        if let Some(before) = before {
            assert_eq!(before.trim(), "[7,null,[\"selected\",\"index\"]]");
            assert_eq!(
                run_javascript(&ws.read("selected.js")).as_deref(),
                Some(before.as_str())
            );
            eprintln!(
                "RUNTIME optional receiver null/non-null value/order verified unchanged: {before}"
            );
        }
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn proven_optional_receiver_without_postfix_is_supported() {
    let source = method_source("await /* keep */ object?.retry()");
    let before = run_javascript(&source);
    let ws = workspace(&source, "js");
    let (gw, diagnostics) = method_gateway(&ws, &source).await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = ws.read("selected.js");
    assert!(
        after.contains("await /* keep */ object?.retry()"),
        "{after}"
    );
    if let Some(before) = before {
        assert_eq!(before.trim(), "[[7],null,[\"selected\"]]");
        assert_eq!(run_javascript(&after).as_deref(), Some(before.as_str()));
        eprintln!("RUNTIME supported optional receiver value/order verified: {before}");
    }
    validated(&diagnostics);
}

#[tokio::test]
async fn proven_method_postfixes_preserve_existing_chain_boundaries() {
    for (expression, expected, value) in [
        (
            "await /* keep */ object.retry()[mark()]",
            "await /* keep */ (await object.retry())[mark()]",
            "[7,null,[\"selected\",\"index\"]]",
        ),
        (
            "await (object?.retry())[mark()]",
            "await (await (object?.retry()))[mark()]",
            "[7,\"TypeError\",[\"selected\",\"index\",\"index\"]]",
        ),
        (
            "(await object?.retry())[mark()]",
            "(await object?.retry())[mark()]",
            "[7,\"TypeError\",[\"selected\",\"index\",\"index\"]]",
        ),
    ] {
        let source = method_source(expression).replace(
            "await run(null)",
            if expression.contains("?.") {
                "await run(null).catch(error => error.name)"
            } else {
                "null"
            },
        );
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = method_gateway(&ws, &source).await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(plan.propagated, 1, "{plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{plan:?}"
        );
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        let after = ws.read("selected.js");
        assert!(after.contains(expected), "{after}");
        if let Some(before) = before {
            assert_eq!(before.trim(), value);
            assert_eq!(run_javascript(&after).as_deref(), Some(before.as_str()));
            eprintln!("RUNTIME supported method chain boundary value/order verified: {before}");
        }
        validated(&diagnostics);
    }
}
