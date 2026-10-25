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
    .expect("runtime controls must execute")
}

#[tokio::test]
async fn yield_scope_expression_arrow_preserves_value_and_order() {
    let source = "const events = [];\nfunction retry() {\n events.push(\"selected\");\n return 2;\n}\nfunction divisor() { events.push(\"divisor\"); return 2; }\nvar yield = 8;\nfunction* outer() { const f = async () => yield / retry() / divisor(); return f; }\nouter().next().value().then(value => console.log(JSON.stringify([String(value), events])));\n";
    let before = script(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(ws.read("selected.js"), source);
    let applied = tool(gw.addr(), &ws, "js", true, false).await;
    let after = script(&ws.read("selected.js"));
    eprintln!(
        "YIELD_SCOPE arrow propagated={} blocked={:?} unmatched={:?} applied={}\nbefore={before:?} after={after:?}\n{}",
        plan.propagated,
        plan.blocked,
        plan.unmatched,
        applied.is_ok(),
        ws.read("selected.js")
    );
    assert_eq!(before.trim(), "[\"2\",[\"selected\",\"divisor\"]]");
    assert_eq!(after, before);
    if plan.unmatched.is_empty() && plan.blocked.is_empty() {
        assert_eq!(plan.propagated, 1, "{plan:?}");
        applied.unwrap();
        assert_eq!(ws.read("selected.js"), planned(&plan, "js"));
        validated(&diagnostics);
    } else {
        assert!(
            plan.blocked
                .iter()
                .chain(&plan.unmatched)
                .any(|site| { site.contains("selected.js:8:") && site.contains("retry") }),
            "{plan:?}"
        );
        assert!(applied.is_err());
        assert_eq!(ws.read("selected.js"), source);
    }
}

#[tokio::test]
async fn yield_scope_template_generator_preserves_regex() {
    let source = "const events = [];\nfunction retry() {\n events.push(\"selected\");\n return 2;\n}\nasync function* gen() { const text = `${yield / retry() /}`; }\n(async () => { const it = gen(); const first = await it.next(); console.log(JSON.stringify([first.value.source, events])); })();\n";
    let before = script(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(ws.read("selected.js"), source);
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = script(&ws.read("selected.js"));
    eprintln!(
        "YIELD_SCOPE template propagated={} blocked={:?} unmatched={:?}\nbefore={before:?} after={after:?}\n{}",
        plan.propagated,
        plan.blocked,
        plan.unmatched,
        ws.read("selected.js")
    );
    assert_eq!(before.trim(), "[\" retry() \",[]]");
    assert_eq!(after, before);
    assert_eq!(plan.propagated, 0, "{plan:?}");
    assert!(plan.blocked.is_empty() && plan.unmatched.is_empty());
    validated(&diagnostics);
}

#[tokio::test]
async fn yield_scope_nested_templates_and_completed_arrows_keep_generator_regexes() {
    for declaration in [
        "async function* gen() { const text = `prefix ${1} ${yield / retry() /}`; }",
        "async function* gen() { const text = `${`${yield / retry() /}`}`; }",
        "function* gen() { const text = `${yield / retry() /}`; }",
        "async function* gen() { const f = () => 1; const text = `${yield / retry() /}`; }",
        "async function* gen() { const f = async () => 1; yield / retry() /; }",
        "const holder = { async *gen() { const text = `${yield / retry() /}`; } }; const gen = () => holder.gen();",
        "const factory = () => (async function* gen() { const text = `${yield / retry() /}`; }); const gen = factory();",
    ] {
        let source = format!(
            "const events = [];\nfunction retry() {{\n events.push(\"selected\");\n return 2;\n}}\n{declaration}\n(async () => {{ const first = await gen().next(); console.log(JSON.stringify([first.value.source, events])); }})();\n"
        );
        let before = script(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(plan.propagated, 0, "{declaration}: {plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{declaration}: {plan:?}"
        );
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert_eq!(script(&ws.read("selected.js")), before, "{declaration}");
        assert!(ws.read("selected.js").contains(declaration));
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn yield_scope_arrow_operands_are_rewritten_or_refused_before_writes() {
    for expression in [
        "(yield / retry() / divisor())",
        "1 + yield / retry() / divisor()",
        "true ? yield / retry() / divisor() : 0",
        "`${0, yield / retry() / divisor()}`",
        "`${{a: 0}} ${yield / retry() / divisor()}`",
        "`${`${yield / retry() / divisor()}`}`",
    ] {
        let source = format!(
            "const events = [];\nfunction retry() {{\n events.push(\"selected\");\n return 2;\n}}\nfunction divisor() {{ events.push(\"divisor\"); return 2; }}\nvar yield = 8;\nfunction* outer() {{ const f = async () => {expression}; return f; }}\nouter().next().value().then(value => console.log(JSON.stringify([String(value), events])));\n"
        );
        let before = script(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        let applied = tool(gw.addr(), &ws, "js", true, false).await;
        let after = script(&ws.read("selected.js"));
        eprintln!(
            "YIELD_SCOPE operand {expression}: propagated={} blocked={:?} unmatched={:?} applied={} before={before:?} after={after:?}",
            plan.propagated,
            plan.blocked,
            plan.unmatched,
            applied.is_ok()
        );
        if plan.unmatched.is_empty() && plan.blocked.is_empty() {
            assert_eq!(plan.propagated, 1, "{expression}: {plan:?}");
            applied.unwrap();
            validated(&diagnostics);
        } else {
            assert!(
                plan.blocked
                    .iter()
                    .chain(&plan.unmatched)
                    .any(|site| { site.contains("selected.js:8:") && site.contains("retry") }),
                "{expression}: {plan:?}"
            );
            assert!(applied.is_err());
            assert_eq!(ws.read("selected.js"), source);
        }
        assert_eq!(after, before, "{expression}");
    }
}

#[tokio::test]
async fn yield_scope_interpolation_keeps_contextual_of_as_a_value() {
    let source = "const events = [];\nfunction retry() {\n events.push(\"selected\");\n return 2;\n}\nasync function run() {\n const of = 8;\n for (const value of `${of / retry() / 2}`) { console.log(JSON.stringify([value, events])); }\n}\nrun();\n";
    let before = script(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    assert_eq!(script(&ws.read("selected.js")), before);
    validated(&diagnostics);
}

#[tokio::test]
async fn yield_scope_asi_multiplication_is_not_a_generator_method() {
    for prefix in [
        "const async = 2; const g = () => 3; async * g()",
        "const static = 2; const g = () => 3; static * g()",
        "const async = 2; const g = () => 3; 1, async * g()",
        "const async = 2; const g = () => 3; { async * g()",
        "const async = 2; const g = () => 3; const text = 'literal'\n { async * g()",
        "const async = 2; const g = () => 3; const text = `literal`\n { async * g()",
        "const async = 2; const g = () => 3; const text = /literal/\n { async * g()",
    ] {
        let close = if prefix.contains("{ async") { "}" } else { "" };
        let source = format!(
            "const events = [];\nfunction retry() {{\n events.push(\"selected\");\n return 2;\n}}\nfunction divisor() {{ events.push(\"divisor\"); return 2; }}\nasync function run() {{\n {prefix}\n {{ const yield = 8; return yield / retry() / divisor(); }}\n {close}\n}}\nrun().then(value => console.log(JSON.stringify([String(value), events])));\n"
        );
        let before = script(&source);
        assert_eq!(before.trim(), "[\"2\",[\"selected\",\"divisor\"]]");
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), source);
        let applied = tool(gw.addr(), &ws, "js", true, false).await;
        let after = script(&ws.read("selected.js"));
        eprintln!(
            "YIELD_SCOPE ASI {prefix}: propagated={} blocked={:?} unmatched={:?} applied={} before={before:?} after={after:?}\n{}",
            plan.propagated,
            plan.blocked,
            plan.unmatched,
            applied.is_ok(),
            ws.read("selected.js")
        );
        assert_eq!(after, before, "{prefix}");
        if plan.unmatched.is_empty() && plan.blocked.is_empty() {
            assert_eq!(plan.propagated, 1, "{prefix}: {plan:?}");
            applied.unwrap();
            validated(&diagnostics);
        } else {
            let expected_site = format!("selected.js:{}:", 9 + prefix.matches('\n').count());
            assert!(
                plan.unmatched.iter().any(|site| {
                    site.contains(&expected_site) && site.contains("ambiguous regex/division")
                }),
                "{prefix}: {plan:?}"
            );
            assert!(applied.is_err());
            assert_eq!(ws.read("selected.js"), source);
        }
    }
}

#[tokio::test]
async fn yield_scope_proven_generator_members_keep_regex_source_and_order() {
    for (declaration, iterator) in [
        (
            "const holder = { async *gen() { yield / retry() /; } };",
            "holder.gen()",
        ),
        (
            "const holder = { value: 1, async *gen() { yield / retry() /; } };",
            "holder.gen()",
        ),
        (
            "class Holder { static *gen() { yield / retry() /; } }",
            "Holder.gen()",
        ),
        (
            "class Holder { field = 1; async *gen() { yield / retry() /; } }",
            "new Holder().gen()",
        ),
        (
            "class Holder { other() {} async *gen() { yield / retry() /; } }",
            "new Holder().gen()",
        ),
    ] {
        let source = format!(
            "const events = [];\nfunction retry() {{\n events.push(\"selected\");\n return 2;\n}}\n{declaration}\n(async () => {{ const first = await ({iterator}).next(); console.log(JSON.stringify([first.value.source, events])); }})();\n"
        );
        let before = script(&source);
        assert_eq!(before.trim(), "[\" retry() \",[]]");
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), source);
        eprintln!(
            "YIELD_SCOPE member {declaration}: propagated={} blocked={:?} unmatched={:?}",
            plan.propagated, plan.blocked, plan.unmatched
        );
        assert_eq!(plan.propagated, 0, "{declaration}: {plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{declaration}: {plan:?}"
        );
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert_eq!(script(&ws.read("selected.js")), before, "{declaration}");
        assert!(ws.read("selected.js").contains(declaration));
        validated(&diagnostics);
    }
}
