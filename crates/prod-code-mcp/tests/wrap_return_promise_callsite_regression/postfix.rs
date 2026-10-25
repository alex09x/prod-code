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
use std::process::Command;
use std::sync::OnceLock;

#[tokio::test]
async fn already_awaited_postfixes_await_the_result_before_the_postfix() {
    for postfix in ["[0]", ".value", "?.value", "()", "!.value"] {
        for trivia in [" ", " /* keep await */\n ", " // keep await\n "] {
            let expression = format!("await{trivia}retry() /* keep postfix */ {postfix}");
            let source = format!(
                "export function retry(): any {{ return [7]; }}\nexport async function run() {{\n    return {expression};\n}}\n"
            );
            let ws = workspace(&source, "ts");
            let (gw, diagnostics) = gateway(&ws, &source, &[], "ts").await;
            let plan = wrap(gw.addr(), &ws, "ts", false, false).await.unwrap();
            let after = planned(&plan, "ts");
            let expected = format!("await{trivia}(await retry()) /* keep postfix */ {postfix}");
            assert_eq!(ws.read("selected.ts"), source);
            tool(gw.addr(), &ws, "ts", true, false).await.unwrap();
            assert_eq!(ws.read("selected.ts"), after);
            eprintln!(
                "postfix {postfix:?}, trivia {trivia:?}: {}",
                after.lines().collect::<Vec<_>>().join(" | ")
            );
            assert!(after.contains(&expected), "expected {expected:?}: {after}");
            assert_eq!(
                after.matches("await").count(),
                source.matches("await").count() + 1
            );
            assert_eq!(plan.propagated, 1);
            assert!(plan.unmatched.is_empty() && plan.blocked.is_empty());
            validated(&diagnostics);
        }
    }
}

#[tokio::test]
async fn one_await_is_preserved_with_comments_grouping_and_identifier_boundaries() {
    for (expression, expected) in [
        ("await /* retain */ retry()", "await /* retain */ retry()"),
        (
            "(await /* retain */ retry())[0]",
            "(await /* retain */ retry())[0]",
        ),
        (
            "retry() /* retain */ [0]",
            "(await retry()) /* retain */ [0]",
        ),
        ("notawait + retry()[0]", "notawait + (await retry())[0]"),
        (
            "await /* retain */ retry /* callee */ ()[0]",
            "await /* retain */ (await retry /* callee */ ())[0]",
        ),
        ("retry() !== null", "await retry() !== null"),
        ("await (retry())[0]", "await (await (retry()))[0]"),
        (
            "await ((/* retain */ retry())) /* postfix */ [0]",
            "await (await ((/* retain */ retry()))) /* postfix */ [0]",
        ),
        ("outer(retry())", "outer(await retry())"),
    ] {
        let source = format!(
            "export function retry(): any {{ return [7]; }}\nexport async function run() {{\n    return {expression};\n}}\n"
        );
        let ws = workspace(&source, "ts");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "ts").await;
        let result = wrap(gw.addr(), &ws, "ts", true, false).await.unwrap();
        let after = ws.read("selected.ts");
        assert!(after.contains(expected), "{after}");
        assert_eq!(result.propagated, 1);
        validated(&diagnostics);
    }
}

#[derive(Clone, Copy)]
struct JavascriptRuntime {
    executable: &'static str,
    flags: &'static [&'static str],
}

fn javascript_runtime() -> Option<JavascriptRuntime> {
    static RUNTIME: OnceLock<Option<JavascriptRuntime>> = OnceLock::new();
    *RUNTIME.get_or_init(|| {
        // Probe a fixed program independently of the refactored source. Once a
        // capable runtime is found, failures in the actual fixture must fail.
        let probe = "export {}; await Promise.resolve(); if (({value: 1})?.value !== 1) throw Error();";
        for runtime in [
            JavascriptRuntime { executable: "node", flags: &["--input-type=module", "-e"] },
            JavascriptRuntime { executable: "nodejs", flags: &["--input-type=module", "-e"] },
            JavascriptRuntime { executable: "bun", flags: &["--eval"] },
            JavascriptRuntime { executable: "deno", flags: &["eval"] },
        ] {
            match Command::new(runtime.executable).args(runtime.flags).arg(probe).output() {
                Ok(output) if output.status.success() => {
                    eprintln!("RUNTIME selected {}: module/await/optional-chain probe passed", runtime.executable);
                    return Some(runtime);
                }
                Ok(output) => eprintln!(
                    "RUNTIME LIMITATION: {} rejected the fixed capability probe: {}",
                    runtime.executable, String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("no stderr")
                ),
                Err(error) => eprintln!("RUNTIME LIMITATION: {} unavailable: {error}", runtime.executable),
            }
        }
        eprintln!("RUNTIME LIMITATION: no capable JavaScript fixture runtime; value/order assertions did not run");
        None
    })
}

pub(super) fn run_javascript(source: &str) -> Option<String> {
    let runtime = javascript_runtime()?;
    let output = Command::new(runtime.executable)
        .args(runtime.flags)
        .arg(source)
        .output()
        .unwrap_or_else(|error| panic!("cannot start {}: {error}", runtime.executable));
    assert!(
        output.status.success(),
        "runtime failed: {}\n{source}",
        String::from_utf8_lossy(&output.stderr)
    );
    Some(String::from_utf8(output.stdout).unwrap())
}

#[tokio::test]
async fn javascript_runtime_preserves_values_and_postfix_evaluation_order() {
    for (value, postfix, expected) in [
        ("[7]", "[mark()]", "7"),
        ("{value: 8}", ".value", "8"),
        ("{value: 9}", "?.value", "9"),
        ("() => 10", "()", "10"),
    ] {
        let source = format!(
            "export function retry() {{\n    events.push(\"selected\");\n    return {value};\n}}\nconst events = [];\nfunction mark() {{ events.push(\"index\"); return 0; }}\nexport async function run() {{\n    return await /* keep */ retry() /* postfix */ {postfix};\n}}\nconsole.log(JSON.stringify([await run(), events]));\n"
        );
        let before = run_javascript(&source);
        let ws = workspace(&source, "js");
        let (gw, diagnostics) = gateway(&ws, &source, &[], "js").await;
        wrap(gw.addr(), &ws, "js", true, false).await.unwrap();
        let after = run_javascript(&ws.read("selected.js"));
        if let Some(before) = before {
            assert_eq!(after.as_deref(), Some(before.as_str()));
            assert!(before.starts_with(&format!("[{expected},")), "{before}");
            eprintln!("RUNTIME verified postfix {postfix}: {before}");
        }
        validated(&diagnostics);
    }
}
