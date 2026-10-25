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
use std::process::Command;

#[tokio::test]
async fn outer_await_context_locally_consumed_postfix_results() {
    let mut failures = Vec::new();
    for (name, value, postfix, rejected) in [
        ("promise index", "[Promise.resolve(7)]", "[0]", false),
        (
            "async method",
            "{async read() { events.push(\"method\"); return 7; }}",
            ".read()",
            false,
        ),
        ("rejecting index", "[pending]", "[0]", true),
        (
            "rejecting async method",
            "{async read() { events.push(\"method\"); throw Error(\"kept\"); }}",
            ".read()",
            true,
        ),
    ] {
        let source = format!(
            "export function retry() {{\n events.push(\"selected\");\n return {value};\n}}\nconst events = [];\nconst pending = Promise.reject(Error(\"kept\")); pending.catch(() => {{}});\nexport async function run() {{\n const value = await retry(){postfix};\n events.push(\"resolved\");\n return value === 7;\n}}\nconst result = await run().catch(error => {{ events.push(\"caught\"); return error.message === \"kept\"; }});\nconsole.log(JSON.stringify([result, events]));\n"
        );
        let before = run_javascript(&source).expect("runtime must execute");
        assert!(before.starts_with("[true,"), "{before}");
        assert!(before.contains(if rejected { "caught" } else { "resolved" }));
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), source);
        let after = planned(&plan, "js");
        let applied = tool(gw.addr(), &ws, "js", true, false).await;
        assert!(applied.is_ok(), "{applied:?}");
        assert_eq!(ws.read("selected.js"), after);
        let runtime = run_javascript(&after).expect("runtime must execute");
        eprintln!(
            "OUTER_AWAIT {name}: preview propagated={} blocked={:?} unmatched={:?}; no-force applied=true; before={before:?} after={runtime:?}\n{after}",
            plan.propagated, plan.blocked, plan.unmatched
        );
        validated(&diagnostics);
        assert_eq!(plan.propagated, 1);
        assert!(plan.blocked.is_empty() && plan.unmatched.is_empty());
        if before != runtime || !after.contains(&format!("await (await retry()){postfix}")) {
            failures.push(name);
        }
    }
    assert!(failures.is_empty(), "lost outer await: {failures:?}");
}

fn commonjs(source: &str) -> String {
    let output = Command::new("node")
        .args(["--input-type=commonjs", "-e", source])
        .output()
        .expect("Node CommonJS runtime must execute");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
async fn outer_await_context_script_division_requires_complete_evidence() {
    let source = "function retry() {\n return 2;\n}\nfunction run() {\n const await = 8;\n return await / retry() / 2;\n}\nconsole.log(String(run()));\n";
    assert_eq!(commonjs(source).trim(), "2");
    let mut failures = Vec::new();
    for force in [false, true] {
        let ws = workspace(source, "js");
        std::fs::write(ws.root().join("package.json"), "{\"type\":\"commonjs\"}").unwrap();
        let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), source);
        let applied = tool(gw.addr(), &ws, "js", true, force).await;
        let after = ws.read("selected.js");
        let runtime = commonjs(&after);
        eprintln!(
            "SCRIPT_AWAIT force={force}: preview propagated={} blocked={:?} unmatched={:?}; apply={applied:?}; original=2 after={runtime:?}; unchanged={}\n{after}",
            plan.propagated,
            plan.blocked,
            plan.unmatched,
            after == source
        );
        validated(&diagnostics);
        if applied.is_ok()
            || after != source
            || plan.unmatched.is_empty()
            || !plan
                .unmatched
                .iter()
                .any(|site| site.contains("selected.js:6:") && site.contains("retry"))
        {
            failures.push(force);
        }
    }
    assert!(
        failures.is_empty(),
        "silently omitted division caller, force={failures:?}"
    );
}

#[tokio::test]
async fn outer_await_context_proven_async_and_module_regexes_remain_literals() {
    for body in [
        "export async function run() { const regex = await / retry() /; return regex.source; }\nconsole.log(await run());",
        "export async function* run() { const regex = await / retry() /; yield regex.source; }\nconsole.log((await run().next()).value);",
        "export const run = async () => await / retry() /;\nconsole.log((await run()).source);",
        "export const run = async () => { const regex = await / retry() /; return regex.source; };\nconsole.log(await run());",
        "const regex = await / retry() /;\nconsole.log(regex.source);",
    ] {
        let source = format!("export function retry() {{ return 2; }}\n{body}\n");
        let before = run_javascript(&source).expect("runtime must execute");
        assert_eq!(before.trim(), "retry()");
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(plan.propagated, 0, "{body}: {plan:?}");
        assert!(
            plan.blocked.is_empty() && plan.unmatched.is_empty(),
            "{body}: {plan:?}"
        );
        assert_eq!(ws.read("selected.js"), source);
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        let after = ws.read("selected.js");
        assert!(after.contains(body), "{after}");
        assert_eq!(run_javascript(&after).as_deref(), Some(before.as_str()));
        validated(&diagnostics);
        eprintln!("AWAIT_GRAMMAR proven regex preserved: {body}");
    }
}

#[tokio::test]
async fn outer_await_context_nested_script_and_generator_values_are_not_operators() {
    for body in [
        "function run_async() { const await = 8; return await / retry() / 2; }\nconsole.log(String(run_async()));",
        "function* run() { const await = 8; return await / retry() / 2; }\nconsole.log(String(run().next().value));",
        "async function outer() {\n function run() { const await = 8; return await / retry() / 2; }\n return run();\n}\nouter().then(value => console.log(String(value)));",
        "globalThis.export = 1;\nconst await = 8;\nconsole.log(String(await / retry() / 2));",
    ] {
        let source = format!("function retry() {{ return 2; }}\n{body}\n");
        assert_eq!(commonjs(&source).trim(), "2");
        let ws = workspace(&source, "js");
        std::fs::write(ws.root().join("package.json"), "{\"type\":\"commonjs\"}").unwrap();
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert!(!plan.unmatched.is_empty(), "{body}: {plan:?}");
        for force in [false, true] {
            let error = tool(gw.addr(), &ws, "js", true, force).await.unwrap_err();
            assert!(error.contains("nothing was written"), "{error}");
            assert_eq!(ws.read("selected.js"), source);
        }
        validated(&diagnostics);
        eprintln!(
            "AWAIT_GRAMMAR Script value refused with unchanged bytes: {body}; {:?}",
            plan.unmatched
        );
    }
}
