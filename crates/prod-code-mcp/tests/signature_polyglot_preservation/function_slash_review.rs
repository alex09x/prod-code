/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::aggregate_review::{execute, snapshot};
use super::support::*;
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::Workspace;

#[tokio::test]
async fn expression_containers_and_unrelated_statements_preserve_ordinary_calls() {
    let mut failures = Vec::new();
    for (file, unrelated) in [
        ("main.js", "const unused = `${0}${{} / 2}`;"),
        (
            "main.jsx",
            "const React = { createElement: (...args) => args }; const unused = <div>{0}{{} / 2}</div>;",
        ),
        ("main.js", "label: {} /[)]/.test(')');"),
    ] {
        for callback in [false, true] {
            for force in [false, true] {
                let callback_source = if callback {
                    "const callback = f;\n"
                } else {
                    ""
                };
                // Repeat the unrelated statement after the selected call too:
                // neither side may become a whole-file lexer prerequisite.
                let source = format!(
                    "function f(a,b) {{ return a*10+b; }}\n{{ {unrelated} }}\nconsole.log(f(1,2));\n{{ {unrelated} }}\n{callback_source}"
                );
                let ws = Workspace::new(&[(file, &source)]);
                let root = &ws.root();
                let label = format!("containers-{file}-{unrelated}-{callback}-{force}");
                snapshot(root, file, &format!("{label}-before"));
                assert_eq!(execute(root, file), "12\n");
                let call = location(root, file, &source, "f(1,2)");
                assert_eq!(
                    call["range"]["start"],
                    serde_json::json!({"line": 2, "character": 12})
                );
                let mut refs = vec![call];
                if callback {
                    let reference = location(root, file, &source, "f;");
                    assert_eq!(
                        reference["range"]["start"],
                        serde_json::json!({"line": 4, "character": 17})
                    );
                    refs.push(reference);
                }
                eprintln!("{label} semantic coordinates: {refs:?}");
                let gw = gateway(refs).await;
                let result = change_with(
                    gw.addr(),
                    root,
                    &root.join(file),
                    1,
                    10,
                    &[Param::Keep("b".into()), Param::Keep("a".into())],
                    &Modifiers::default(),
                    true,
                    force,
                )
                .await;
                eprintln!("{label} public result: {result:?}");
                let actual = snapshot(root, file, &format!("{label}-after"));
                assert_eq!(execute(root, file), "12\n");
                let valid = if callback {
                    result.is_err_and(|error| error.to_string().contains("nothing was written"))
                        && actual.as_bytes() == source.as_bytes()
                } else {
                    result.is_ok()
                        && actual
                            == source
                                .replace("f(a,b)", "f(b, a)")
                                .replace("f(1,2)", "f(2, 1)")
                };
                if !valid {
                    failures.push(label);
                }
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
}

#[tokio::test]
async fn bare_block_regex_preserves_ordinary_reorder_and_callback_refusal() {
    let source = "function f(a,b) { return a*10+b; }\n{ const unused = 0; } /[)]/.test(\")\");\nconsole.log(f(1,2));\n";
    for callback in [false, true] {
        let source = if callback {
            format!("{source}const callback = f;\n")
        } else {
            source.to_string()
        };
        for force in [false, true] {
            let ws = Workspace::new(&[("main.js", &source)]);
            let root = &ws.root();
            let label = format!("bare-block-callback-{callback}-force-{force}");
            snapshot(root, "main.js", &format!("{label}-before"));
            assert_eq!(execute(root, "main.js"), "12\n");
            let call = location(root, "main.js", &source, "f(1,2)");
            assert_eq!(
                call["range"]["start"],
                serde_json::json!({"line": 2, "character": 12})
            );
            let mut refs = vec![call];
            if callback {
                let reference = location(root, "main.js", &source, "f;");
                assert_eq!(
                    reference["range"]["start"],
                    serde_json::json!({"line": 3, "character": 17})
                );
                refs.push(reference);
            }
            eprintln!(
                "{label} semantic coordinates: {:?}",
                refs.iter().map(|r| &r["range"]).collect::<Vec<_>>()
            );
            let gw = gateway(refs).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join("main.js"),
                1,
                10,
                &[Param::Keep("b".into()), Param::Keep("a".into())],
                &Modifiers::default(),
                true,
                force,
            )
            .await;
            eprintln!("{label} public result: {result:?}");
            let actual = snapshot(root, "main.js", &format!("{label}-after"));
            assert_eq!(execute(root, "main.js"), "12\n");
            if callback {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("nothing was written")
                );
                assert_eq!(actual.as_bytes(), source.as_bytes());
            } else {
                result.unwrap();
                assert_eq!(
                    actual,
                    source
                        .replace("f(a,b)", "f(b, a)")
                        .replace("f(1,2)", "f(2, 1)")
                );
            }
        }
    }
}

#[tokio::test]
async fn function_expression_division_preserves_ordinary_reorder() {
    let source = "function f(a,b) { return a*10+b; }\nconst unused = function() {} / 2;\nconsole.log(f(1,2));\n";
    let ws = Workspace::new(&[("main.js", source)]);
    let root = &ws.root();
    snapshot(root, "main.js", "exact-function-division-before");
    assert_eq!(execute(root, "main.js"), "12\n");
    let gw = gateway(vec![location(root, "main.js", source, "f(1,2)")]).await;
    let result = change_with(
        gw.addr(),
        root,
        &root.join("main.js"),
        1,
        10,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        false,
    )
    .await;
    eprintln!("exact-function-division public result: {result:?}");
    let actual = snapshot(root, "main.js", "exact-function-division-after");
    assert_eq!(execute(root, "main.js"), "12\n");
    result.unwrap();
    assert_eq!(
        actual,
        source
            .replace("f(a,b)", "f(b, a)")
            .replace("f(1,2)", "f(2, 1)")
    );
}

#[tokio::test]
async fn function_slash_context_preserves_reorder_and_both_async_directions() {
    let mut failures = Vec::new();
    for (file, unrelated) in [
        ("main.js", "const unused = function() {} / 2;"),
        ("main.js", "const unused = function named() {} / 2;"),
        ("main.js", "const unused = function* named() {} / 2;"),
        ("main.js", "const unused = async function() {} / 2;"),
        ("main.js", "const unused = async function* named() {} / 2;"),
        ("main.js", "const unused = [function() {} / 2];"),
        ("main.js", "const unused = true ? function() {} / 2 : 0;"),
        ("main.js", "function other() {} /[)]/.test(')');"),
        ("main.js", "async function other() {} /[)]/.test(')');"),
        ("main.js", "function* other() {} /[)]/.test(')');"),
        (
            "main.js",
            "const x = 1\nfunction other() {} /[)]/.test(')');",
        ),
        (
            "main.ts",
            "// @ts-ignore: intentional JavaScript coercion\nconst unused = function() {} / 2;",
        ),
        ("main.jsx", "const unused = function() {} / 2;"),
        (
            "main.tsx",
            "// @ts-ignore: intentional JavaScript coercion\nconst unused = async function named() {} / 2;",
        ),
    ] {
        for asyncness in [None, Some(true), Some(false)] {
            let modifier = if asyncness == Some(false) {
                "async "
            } else {
                ""
            };
            let awaited = if asyncness == Some(false) {
                "await "
            } else {
                ""
            };
            let source = format!(
                "{modifier}function f(a,b) {{ return a*10+b; }}\n{unrelated}\nasync function main() {{ console.log({awaited}f(1,2)); }}\nmain();\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            let label = format!("function-slash-{file}-{unrelated}-{asyncness:?}");
            snapshot(root, file, &format!("{label}-before"));
            assert_eq!(execute(root, file), "12\n");
            let gw = gateway(vec![location(root, file, &source, "f(1,2)")]).await;
            let params = if asyncness.is_none() {
                vec![Param::Keep("b".into()), Param::Keep("a".into())]
            } else {
                vec![Param::Keep("a".into()), Param::Keep("b".into())]
            };
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                1,
                10,
                &params,
                &Modifiers {
                    asyncness,
                    ..Default::default()
                },
                true,
                false,
            )
            .await;
            eprintln!("{label} public result: {result:?}");
            let actual = snapshot(root, file, &format!("{label}-after"));
            assert_eq!(execute(root, file), "12\n");
            let expected = match asyncness {
                None => source
                    .replace("f(a,b)", "f(b, a)")
                    .replace("f(1,2)", "f(2, 1)"),
                Some(true) => source
                    .replace("function f(a,b)", "async function f(a, b)")
                    .replace("f(1,2)", "(await f(1,2))"),
                Some(false) => source
                    .replace("async function f(a,b)", "function f(a, b)")
                    .replace("await f(1,2)", "f(1,2)"),
            };
            if result.is_err() || actual != expected {
                failures.push(format!("{label}: actual={actual:?}, expected={expected:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
