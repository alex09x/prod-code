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
    // Indirect eval uses the Script grammar and does not inherit module strictness.
    run_javascript(&format!(
        "await (0,eval)({});",
        serde_json::to_string(source).unwrap()
    ))
    .expect("runtime controls must execute")
}

#[tokio::test]
async fn residual_omission_namespace_comment_preserves_value_and_order() {
    let source = "import * as own from './selected.js';\nexport const events = [];\nexport function retry() {\n    events.push(\"selected\");\n    return 2;\n}\nfunction divisor() { events.push(\"divisor\"); return 2; }\nexport async function run() {\n    /*\n    const own = fake;\n    */\n    return own.retry() / divisor();\n}\n";
    let ws = workspace(source, "js");
    let harness = format!(
        "import {{ run, events }} from {}; console.log(JSON.stringify([String(await run()), events]));",
        serde_json::to_string(
            url::Url::from_file_path(ws.root().join("selected.js"))
                .unwrap()
                .as_str()
        )
        .unwrap()
    );
    let before = run_javascript(&harness).expect("namespace runtime must execute");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(ws.read("selected.js"), source);
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = run_javascript(&harness).expect("namespace runtime must execute");
    eprintln!(
        "RESIDUAL namespace propagated={} unmatched={:?}\nbefore={before:?} after={after:?}\n{}",
        plan.propagated,
        plan.unmatched,
        ws.read("selected.js")
    );
    validated(&diagnostics);
    assert_eq!(before.trim(), "[\"1\",[\"selected\",\"divisor\"]]");
    assert_eq!(after, before);
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
    assert_eq!(ws.read("selected.js"), planned(&plan, "js"));
}

#[tokio::test]
async fn residual_omission_script_yield_preserves_value_and_order() {
    let source = "const events = [];\nfunction retry() {\n    events.push(\"selected\");\n    return 2;\n}\nfunction divisor() { events.push(\"divisor\"); return 2; }\nasync function run() {\n    const yield = 8;\n    return yield / retry() / divisor();\n}\nrun().then(value => console.log(JSON.stringify([String(value), events])));\n";
    let before = script(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    assert_eq!(ws.read("selected.js"), source);
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    let after = script(&ws.read("selected.js"));
    eprintln!(
        "RESIDUAL yield propagated={} unmatched={:?}\nbefore={before:?} after={after:?}\n{}",
        plan.propagated,
        plan.unmatched,
        ws.read("selected.js")
    );
    validated(&diagnostics);
    assert_eq!(before.trim(), "[\"2\",[\"selected\",\"divisor\"]]");
    assert_eq!(after, before);
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
    assert_eq!(ws.read("selected.js"), planned(&plan, "js"));
}

#[tokio::test]
async fn namespace_shadow_proof_uses_executable_bindings() {
    for trivia in [
        "/*\n const own = fake;\n */",
        "// const own = fake;",
        "const text = \"const own = fake;\";",
        "const text = `const own = fake;`;",
        "const pattern = /const own = fake;/;",
    ] {
        let source = format!(
            "import * as own from './selected.js';\nexport function retry() {{ return 2; }}\nexport async function run() {{\n {trivia}\n return own.retry() / 2;\n}}\n"
        );
        let ws = workspace(&source, "js");
        let local = "import * as own from './selected.js';\nimport * as other from './other.js';\nexport async function run() {\n const own = other;\n return own.retry() / 2;\n}\n";
        let parameter = "import * as own from './selected.js';\nexport async function run(own) { return own.retry() / 2; }\n";
        std::fs::write(ws.root().join("local.js"), local).unwrap();
        std::fs::write(ws.root().join("parameter.js"), parameter).unwrap();
        std::fs::write(
            ws.root().join("other.js"),
            "export function retry() { return 12; }\n",
        )
        .unwrap();
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(plan.propagated, 1, "{trivia}: {plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{trivia}: {plan:?}"
        );
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert!(ws.read("selected.js").contains("await own.retry() / 2"));
        assert_eq!(ws.read("local.js"), local);
        assert_eq!(ws.read("parameter.js"), parameter);
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn generator_yield_regex_and_script_identifier_controls() {
    for declaration in [
        "function* pattern() { yield / retry() /; }",
        "function* pattern() { if (true) { yield /* expression */ / retry() /; } }",
        "const holder = { *pattern() { yield / retry() /; } };",
        "class Holder { *pattern() { yield / retry() /; } }",
        "function* outer() { function pattern() { const yield = 8; return yield / 2; } yield / retry() /; }",
    ] {
        let source = format!(
            "const events = [];\nfunction retry() {{\n events.push(\"selected\");\n return 2;\n}}\n{declaration}\nasync function run() {{ return retry() / 2; }}\nrun().then(value => console.log(JSON.stringify([String(value), events])));\n"
        );
        let before = script(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        assert_eq!(plan.propagated, 1, "{declaration}: {plan:?}");
        assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert!(ws.read("selected.js").contains(declaration));
        assert_eq!(script(&ws.read("selected.js")), before);
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn contextual_yield_division_keeps_executable_callers() {
    for (declaration, statement) in [
        ("const yield = 8;", "return yield / retry() / 2;"),
        ("let yield = 8;", "return yield /* value */ / retry() / 2;"),
        ("var yield = 8;", "return (yield / retry() / 2);"),
        (
            "const yield = 8;",
            "if (true) { return yield / retry() / 2; }",
        ),
        (
            "const yield = 8;",
            "return (() => { return yield / retry() / 2; })();",
        ),
        ("const yield = 8;", "return holder.yield / retry() / 2;"),
    ] {
        let source = format!(
            "function retry() {{ return 2; }}\nconst holder = {{ yield: 8 }};\nasync function run() {{\n {declaration}\n {statement}\n}}\nrun().then(value => console.log(String(value)));\n"
        );
        let before = script(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        eprintln!("YIELD {declaration} {statement}: {plan:?}");
        // An ordinary synchronous arrow can need propagation beyond its own
        // boundary; it must remain visible rather than silently disappearing.
        if plan.blocked.is_empty() && plan.unmatched.is_empty() {
            assert_eq!(plan.propagated, 1, "{plan:?}");
            tool(gw.addr(), &ws, "js", true, false).await.unwrap();
            assert_eq!(script(&ws.read("selected.js")), before);
        } else {
            assert!(
                plan.blocked
                    .iter()
                    .chain(&plan.unmatched)
                    .any(|site| site.contains("selected.js:5:") && site.contains("retry")),
                "{plan:?}"
            );
            assert!(tool(gw.addr(), &ws, "js", true, false).await.is_err());
            assert_eq!(ws.read("selected.js"), source);
        }
        validated(&diagnostics);
    }
}

#[tokio::test]
async fn uncertain_yield_method_context_is_unmatched_before_writes() {
    let source = "function retry() { return 2; }\nconst holder = { async run() {\n const yield = 8;\n return yield / retry() / 2;\n} };\nholder.run().then(value => console.log(String(value)));\n";
    assert_eq!(script(source).trim(), "2");
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    eprintln!("YIELD ambiguous method: {plan:?}");
    assert!(
        plan.unmatched
            .iter()
            .any(|site| site.contains("ambiguous regex/division")),
        "{plan:?}"
    );
    assert!(tool(gw.addr(), &ws, "js", true, false).await.is_err());
    assert_eq!(ws.read("selected.js"), source);
    validated(&diagnostics);
}
