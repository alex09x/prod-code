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
async fn contextual_of_division_preserves_selected_callers() {
    for statement in [
        "return of / retry() / 2;",
        "return of /* value */ / retry() / 2;",
        "return (of / retry() / 2);",
        "for (; of / retry() / 2 > 0;) { return 2; }",
        "for (of / retry() / 2; ;) { return 2; }",
        "for (let value = of / retry() / 2; ;) { return value; }",
        "for (const value of [of / retry() / 2]) { return value; }",
    ] {
        let source = format!(
            "export function retry() {{\n    events.push(\"selected\");\n    return 2;\n}}\nconst events = [];\nexport async function run() {{\n    const of = 8;\n    {statement}\n}}\nconsole.log(JSON.stringify([await run(), events]));\n"
        );
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        let after = planned(&plan, "js");
        eprintln!(
            "contextual of {statement}: propagated={} unmatched={:?}\n{after}",
            plan.propagated, plan.unmatched
        );
        assert_eq!(ws.read("selected.js"), source, "planning must not write");
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), after);
        validated(&diagnostics);
        let runtime = run_javascript(&after);
        if let Some(before) = before {
            eprintln!("RUNTIME contextual of before={before:?} after={runtime:?}");
            assert_eq!(before.trim(), "[2,[\"selected\"]]");
            assert_eq!(runtime.as_deref(), Some(before.as_str()), "{statement}");
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
async fn for_of_regex_literals_do_not_become_selected_callers() {
    for header in [
        "const value of",
        "let value of",
        "var value of",
        "value of",
        "const { value } of",
        "const [value] of",
        "const value /* binding */ of",
        "of of",
        "const of of",
        "holder.of of",
        "holder.return of",
    ] {
        let source = format!(
            "export function retry() {{ return 7; }}\nasync function pattern() {{\n    var value, of;\n    const holder = {{}};\n    for ({header} / retry() /) {{}}\n    for await ({header} / retry() /) {{}}\n}}\nexport async function run() {{ return retry(); }}\nconsole.log(JSON.stringify(await run()));\n"
        );
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        let plan = wrap(gw.addr(), &ws, "js", false, false).await.unwrap();
        let after = planned(&plan, "js");
        assert_eq!(plan.propagated, 1, "{header}: {plan:?}");
        assert!(
            plan.unmatched.is_empty() && plan.blocked.is_empty(),
            "{plan:?}"
        );
        assert!(
            after.contains(&format!("for ({header} / retry() /)")),
            "{after}"
        );
        assert!(
            after.contains(&format!("for await ({header} / retry() /)")),
            "{after}"
        );
        assert_eq!(ws.read("selected.js"), source);
        tool(gw.addr(), &ws, "js", true, false).await.unwrap();
        assert_eq!(ws.read("selected.js"), after);
        validated(&diagnostics);
        let runtime = run_javascript(&after);
        if let Some(before) = before {
            assert_eq!(before.trim(), "7");
            assert_eq!(runtime.as_deref(), Some(before.as_str()));
            eprintln!("RUNTIME for-of regex {header}: {before}");
        }
    }
}
