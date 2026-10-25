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
async fn keyword_property_division_retains_the_selected_caller() {
    for expression in [
        "ops.await / retry() / 2",
        "ops. /* property */ await / retry() / 2",
        "ops?.await / retry() / 2",
        "ops.return / retry() / 2",
        "this.#await / retry() / 2",
    ] {
        let source = format!(
            "export function retry() {{\n    events.push(\"selected\");\n    return 2;\n}}\nconst events = [];\nconst ops = {{ await: 8, return: 8 }};\nclass Runner {{\n    #await = 8;\n    async run() {{\n        return {expression};\n    }}\n}}\nconsole.log(JSON.stringify([await new Runner().run(), events]));\n"
        );
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        let after = planned(&plan, "js");
        eprintln!(
            "property division {expression}: propagated={} unmatched={:?}\n{after}",
            plan.propagated, plan.unmatched
        );
        assert_eq!(ws.read("selected.js"), source, "planning must not write");
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), after);
        validated(&diagnostics);
        let runtime = run_javascript(&after);
        if let Some(before) = before {
            assert_eq!(before.trim(), "[2,[\"selected\"]]");
            assert_eq!(runtime.as_deref(), Some(before.as_str()), "{expression}");
            eprintln!("RUNTIME property division verified: {before}");
        }
        assert_eq!(plan.propagated, 1, "{plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{plan:?}"
        );
        assert!(after.contains("/ await retry() / 2"), "{after}");
    }
}

#[tokio::test]
async fn private_await_method_keeps_its_argument_boundary() {
    for (expression, expected) in [
        ("this.#await(retry())[0]", "this.#await(await retry())[0]"),
        (
            "this.#await /* argument */ ((retry()))[0]",
            "this.#await /* argument */ ((await retry()))[0]",
        ),
        (
            "this.#await(retry())[mark()]",
            "this.#await(await retry())[mark()]",
        ),
    ] {
        let source = format!(
            "export function retry() {{\n    events.push(\"selected\");\n    return [7];\n}}\nconst events = [];\nfunction mark() {{ events.push(\"index\"); return 0; }}\nclass Runner {{\n    #await(value) {{ events.push(\"method\"); return value; }}\n    async run() {{\n        return {expression};\n    }}\n}}\nconsole.log(JSON.stringify([await new Runner().run(), events]));\n"
        );
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        let after = planned(&plan, "js");
        eprintln!("private method {expression}:\n{after}");
        assert_eq!(ws.read("selected.js"), source, "planning must not write");
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), after);
        validated(&diagnostics);
        let runtime = run_javascript(&after);
        if let Some(before) = before {
            let expected_value = if expression.contains("mark()") {
                "[7,[\"selected\",\"method\",\"index\"]]"
            } else {
                "[7,[\"selected\",\"method\"]]"
            };
            assert_eq!(before.trim(), expected_value);
            assert_eq!(runtime.as_deref(), Some(before.as_str()));
            eprintln!("RUNTIME private await method verified: {before}");
        }
        assert_eq!(plan.propagated, 1, "{plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{plan:?}"
        );
        assert!(after.contains(expected), "{after}");
    }
}

#[tokio::test]
async fn genuine_await_and_regex_literals_keep_their_token_roles() {
    let source = "export function retry() { return [7]; }\nfunction pattern() { return / retry() /; }\nexport async function run() {\n    const regex = await / retry() /;\n    return (await /* selected */ retry())[0];\n}\nconsole.log(JSON.stringify([await run(), pattern().source]));\n";
    let before = run_javascript(source);
    let ws = workspace(source, "js");
    let (gw, diagnostics) = gateway(&ws, source, &[], "js").await;
    let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
    let after = planned(&plan, "js");
    assert_eq!(plan.propagated, 1, "{plan:?}");
    assert!(
        plan.unmatched.is_empty() && plan.blocked.is_empty(),
        "{plan:?}"
    );
    assert!(after.contains("return / retry() /;"), "{after}");
    assert!(
        after.contains("const regex = await / retry() /;"),
        "{after}"
    );
    assert!(
        after.contains("(await /* selected */ retry())[0]"),
        "{after}"
    );
    tool(gw.addr(), &ws, "js", true, false).await.unwrap();
    assert_eq!(ws.read("selected.js"), after);
    validated(&diagnostics);
    let runtime = run_javascript(&after);
    if let Some(before) = before {
        assert_eq!(before.trim(), "[7,\" retry() \"]");
        assert_eq!(runtime.as_deref(), Some(before.as_str()));
        eprintln!("RUNTIME await/regex token control verified: {before}");
    }
}
