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

fn script(source: &str) -> String {
    run_javascript(&format!(
        "await (0,eval)({});",
        serde_json::to_string(source).unwrap()
    ))
    .expect("Script runtime control must execute")
}

#[tokio::test]
async fn residual_omission_nested_function_parameters_refuse_before_writes() {
    for declaration in [
        "function inner(x = yield / retry() / divisor()) { return x; }",
        "const inner = function(x = yield / retry() / divisor()) { return x; };",
        "const inner = function /* header */ named(x = ((yield / retry() / divisor()))) { return x; };",
        "function inner({x = yield / retry() / divisor()} = {}) { return x; }",
    ] {
        let source = format!(
            "const events = [];\nfunction retry() {{\n events.push(\"selected\");\n return 2;\n}}\nfunction divisor() {{ events.push(\"divisor\"); return 2; }}\nvar yield = 8;\nfunction* outer() {{ {declaration} return inner(); }}\nconsole.log(JSON.stringify([String(outer().next().value), events]));\n"
        );
        let before = script(&source);
        let unawaited = script(&source.replacen("function retry()", "async function retry()", 1));
        let ws = workspace(&source, "js");
        let (gw, _) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), source, "planning must not write");
        let applied = tool(gw.addr(), &ws, "js", true, false).await;
        let after = script(&ws.read("selected.js"));
        eprintln!(
            "PARAMETER_SCOPE {declaration}: propagated={} blocked={:?} unmatched={:?} applied={applied:?}\nbefore={before:?} declaration_only={unawaited:?} after={after:?}\n{}",
            plan.propagated,
            plan.blocked,
            plan.unmatched,
            ws.read("selected.js")
        );
        assert_eq!(before.trim(), "[\"2\",[\"selected\",\"divisor\"]]");
        assert_eq!(unawaited.trim(), "[\"NaN\",[\"selected\",\"divisor\"]]");
        assert!(
            plan.unmatched
                .iter()
                .any(|site| site.contains("selected.js:8:") && site.contains("retry")),
            "the parameter caller must retain unmatched evidence: {plan:?}"
        );
        assert!(applied.is_err(), "incomplete migration must refuse writes");
        assert_eq!(ws.read("selected.js"), source);
        assert_eq!(after, before, "parameter value and evaluation order");
    }
}

#[tokio::test]
async fn parameter_scope_nested_generator_body_keeps_its_own_grammar() {
    let source = "function retry() { return 2; }\nvar yield = 8;\nfunction* outer() { function inner(x = function*() { yield / retry() /; }) { return x; } return inner(); }\nconsole.log(outer().next().value().next().value.source);\n";
    let before = script(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(ws.read("selected.js"), source);
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = script(&ws.read("selected.js"));
    eprintln!(
        "PARAMETER_SCOPE generator control: propagated={} blocked={:?} unmatched={:?} before={before:?} after={after:?}",
        plan.propagated, plan.blocked, plan.unmatched
    );
    assert_eq!(before.trim(), "retry()");
    assert_eq!(after, before);
    assert_eq!(plan.propagated, 0);
    assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
    validated(&diagnostics);
}
