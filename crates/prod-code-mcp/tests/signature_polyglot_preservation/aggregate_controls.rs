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
async fn python_keywords_bind_by_name_and_comparisons_stay_positional() {
    for (arguments, request, parameters, rewritten, expected) in [
        (
            "b=2, a=1",
            vec![Param::Keep("b".into()), Param::Keep("a".into())],
            "b, a",
            "b=2, a=1",
            "12\n",
        ),
        (
            "1, b=2",
            vec![Param::Keep("b".into()), Param::Keep("a".into())],
            "b, a",
            "b=2, a=1",
            "12\n",
        ),
        (
            "1, effect() != 1",
            vec![
                Param::Keep("a".into()),
                Param::Keep("b".into()),
                Param::Add {
                    name: "extra".into(),
                    ty: String::new(),
                    value: "2".into(),
                },
            ],
            "a, b, extra = 2",
            "1, effect() != 1, 2",
            "effect\n11\n",
        ),
    ] {
        let source = format!(
            "def f(a, b):\n    return a*10+b\ndef effect():\n    print('effect')\n    return 0\nprint(f({arguments}))\n"
        );
        let ws = Workspace::new(&[("main.py", &source)]);
        let root = &ws.root();
        let label = format!("python-positive-{arguments}");
        snapshot(root, "main.py", &format!("{label}-before"));
        assert_eq!(execute(root, "main.py"), expected);
        let call = format!("f({arguments})");
        let gw = gateway(vec![location(root, "main.py", &source, &call)]).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.py"),
            1,
            5,
            &request,
            &Modifiers::default(),
            true,
            false,
        )
        .await;
        eprintln!("{label} public result: {result:?}");
        let actual = snapshot(root, "main.py", &format!("{label}-after"));
        result.unwrap();
        assert_eq!(
            actual,
            source
                .replace("def f(a, b)", &format!("def f({parameters})"))
                .replace(&call, &format!("f({rewritten})"))
        );
        assert_eq!(execute(root, "main.py"), expected);
    }
}

#[tokio::test]
async fn jsx_argument_span_is_shared_with_async_edits() {
    for file in ["main.jsx", "main.tsx"] {
        for enable in [true, false] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let source = format!(
                "{modifier}function f(x) {{ return x; }}\nconst React = {{createElement: (...args) => args}};\nasync function main() {{ console.log(JSON.stringify({awaited}f(<span>)</span>))); }}\nmain();\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            let label = format!("jsx-async-argument-{file}-{enable}");
            snapshot(root, file, &format!("{label}-before"));
            assert_eq!(execute(root, file), "[\"span\",null,\")\"]\n");
            let gw = gateway(vec![location(root, file, &source, "f(<span>)</span>)")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                1,
                if enable { 10 } else { 16 },
                &[Param::Keep("x".into())],
                &Modifiers {
                    asyncness: Some(enable),
                    ..Default::default()
                },
                true,
                false,
            )
            .await;
            eprintln!("{label} public result: {result:?}");
            let actual = snapshot(root, file, &format!("{label}-after"));
            result.unwrap();
            let expected = if enable {
                source
                    .replace("function f", "async function f")
                    .replace("f(<span>)</span>)", "(await f(<span>)</span>))")
            } else {
                source
                    .replace("async function f", "function f")
                    .replace("await f(<span>)</span>)", "f(<span>)</span>)")
            };
            assert_eq!(actual, expected);
            assert_eq!(execute(root, file), "[\"span\",null,\")\"]\n");
        }
    }
}

#[tokio::test]
async fn actual_module_specifiers_do_not_hide_same_line_expression_calls() {
    let source = "function f(x) { return x; }\nexport { f as selected }; const text = ' from require(import';\nconst React = {createElement: (...args) => args};\nasync function main() { const view = <div title={text}>{f(1)}</div>; console.log(JSON.stringify(view)); }\nmain();\n";
    let ws = Workspace::new(&[("main.tsx", source)]);
    let root = &ws.root();
    snapshot(root, "main.tsx", "module-specifier-before");
    let before = execute(root, "main.tsx");
    assert_eq!(before, "[\"div\",{\"title\":\" from require(import\"},1]\n");
    let gw = gateway(vec![
        location(root, "main.tsx", source, "f as selected"),
        location(root, "main.tsx", source, "f(1)"),
    ])
    .await;
    let result = change_with(
        gw.addr(),
        root,
        &root.join("main.tsx"),
        1,
        10,
        &[Param::Keep("x".into())],
        &Modifiers {
            asyncness: Some(true),
            ..Default::default()
        },
        true,
        false,
    )
    .await;
    eprintln!("module-specifier public result: {result:?}");
    let actual = snapshot(root, "main.tsx", "module-specifier-after");
    result.unwrap();
    assert_eq!(
        actual,
        source
            .replace("function f", "async function f")
            .replace("f(1)", "(await f(1))")
    );
    assert_eq!(execute(root, "main.tsx"), before);
}
