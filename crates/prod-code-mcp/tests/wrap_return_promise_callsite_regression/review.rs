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

#[tokio::test]
async fn tagged_templates_remain_unmatched_and_cannot_write() {
    let source = "export function retry(): any { return () => [7]; }\nexport async function run() {\n    return retry`text`()[0];\n}\n";
    let ws = workspace(source, "ts");
    let (gw, diagnostics) = gateway(&ws, source, &[], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    eprintln!(
        "tag plan: propagated={} unmatched={:?}",
        plan.propagated, plan.unmatched
    );
    assert!(!plan.unmatched.is_empty(), "{plan:?}");
    for force in [false, true] {
        assert!(tool(gw.addr(), &ws, "ts", true, force).await.is_err());
        assert_eq!(ws.read("selected.ts"), source);
    }
    validated(&diagnostics);
}

#[tokio::test]
async fn await_property_names_and_identifier_suffixes_are_not_operators() {
    for (expression, expected) in [
        ("object.await(retry())[0]", "object.await(await retry())[0]"),
        ("object.await(retry())", "object.await(await retry())"),
        (
            "object?.await((retry()))[0]",
            "object?.await((await retry()))[0]",
        ),
        ("$await(retry())[0]", "$await(await retry())[0]"),
        (
            "a\u{301}await((retry()))[0]",
            "a\u{301}await((await retry()))[0]",
        ),
    ] {
        let source = format!(
            "export function retry(): any {{ return [7]; }}\nexport async function run() {{\n    return {expression};\n}}\n"
        );
        let ws = workspace(&source, "ts");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "ts").await;
        let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
        let after = planned(&plan, "ts");
        eprintln!("operator {expression}: {after}");
        assert!(after.contains(expected), "{after}");
        assert_eq!(plan.propagated, 1);
        tool(gw.addr(), &ws, "ts", true, false).await.unwrap();
        assert_eq!(ws.read("selected.ts"), after);
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn masked_division_operands_are_never_silently_lost() {
    let source = "export function retry(): number { return 2; }\nexport async function run() {\n    return { valueOf() { return 8; } } / retry() / 2;\n}\n";
    let ws = workspace(source, "ts");
    let (gw, diagnostics) = gateway(&ws, source, &[], "ts").await;
    let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
    eprintln!(
        "division plan: propagated={} unmatched={:?}",
        plan.propagated, plan.unmatched
    );
    assert!(
        plan.propagated == 1 || !plan.unmatched.is_empty(),
        "{plan:?}"
    );
    if plan.unmatched.is_empty() {
        tool(gw.addr(), &ws, "ts", true, false).await.unwrap();
        assert!(ws.read("selected.ts").contains("/ await retry() /"));
    } else {
        for force in [false, true] {
            assert!(tool(gw.addr(), &ws, "ts", true, force).await.is_err());
            assert_eq!(ws.read("selected.ts"), source);
        }
    }
    validated(&diagnostics);
}

#[tokio::test]
async fn star_as_namespace_imports_keep_their_selected_callers() {
    for clause in ["*as own", "* as own", "*/* star */as/* alias */own"] {
        let source = "export function retry(): number { return 7; }\n";
        let ws = workspace(source, "ts");
        let caller = format!(
            "import {clause} from './selected';\nexport async function run() {{\n    return own.retry();\n}}\n"
        );
        std::fs::write(ws.root().join("caller.ts"), &caller).unwrap();
        let (gw, diagnostics) = gateway(&ws, source, &[], "ts").await;
        let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
        eprintln!(
            "namespace {clause}: propagated={} unmatched={:?}",
            plan.propagated, plan.unmatched
        );
        assert_eq!(plan.propagated, 1, "{plan:?}");
        tool(gw.addr(), &ws, "ts", true, false).await.unwrap();
        assert!(ws.read("caller.ts").contains("await own.retry()"));
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn runtime_method_named_await_preserves_values_and_order() {
    let source = "export function retry() {\n    events.push(\"selected\");\n    return [7];\n}\nconst events = [];\nconst object = { await(value) { events.push(\"method\"); return value; } };\nexport async function run() {\n    return object.await(retry())[0];\n}\nconsole.log(JSON.stringify([await run(), events]));\n";
    let before = run_javascript(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    wrap(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = run_javascript(&ws.read("selected.js"));
    if let Some(before) = before {
        assert_eq!(before.trim(), "[7,[\"selected\",\"method\"]]");
        assert_eq!(after.as_deref(), Some(before.as_str()));
        eprintln!("RUNTIME method await verified: {before}");
    }
    validated(&diagnostics);
}

#[tokio::test]
async fn executable_calls_survive_literals_regexes_and_ordinary_division() {
    let source = "export function retry() { return 2; }\nexport async function run() {\n    const text = \"retry()\";\n    const regex = /retry()/;\n    const template = `retry()`;\n    return 8 / retry() / 2;\n}\nconsole.log(await run());\n";
    let before = run_javascript(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(
        plan.unmatched.is_empty() && plan.blocked.is_empty(),
        "{plan:?}"
    );
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = ws.read("selected.js");
    assert!(after.contains("const text = \"retry()\";"), "{after}");
    assert!(after.contains("const regex = /retry()/;"), "{after}");
    assert!(after.contains("const template = `retry()`;"), "{after}");
    assert!(after.contains("8 / await retry() / 2"), "{after}");
    let runtime = run_javascript(&after);
    if let Some(before) = before {
        assert_eq!(before.trim(), "2");
        assert_eq!(runtime.as_deref(), Some(before.as_str()));
        eprintln!("RUNTIME literal/division control verified: {before}");
    }
    validated(&diagnostics);
}

#[tokio::test]
async fn template_interpolation_callers_keep_the_proven_async_owner() {
    let source = "export function retry() { return 2; }\nexport async function run() {\n    return `${retry()}`;\n}\nconsole.log(await run());\n";
    let before = run_javascript(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(
        plan.blocked.is_empty() && plan.unmatched.is_empty(),
        "{plan:?}"
    );
    assert_eq!(ws.read("selected.js"), source);
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    assert!(ws.read("selected.js").contains("`${await retry()}`"));
    if let Some(before) = before {
        assert_eq!(
            run_javascript(&ws.read("selected.js")).as_deref(),
            Some(before.as_str())
        );
    }
    validated(&diagnostics);
}
