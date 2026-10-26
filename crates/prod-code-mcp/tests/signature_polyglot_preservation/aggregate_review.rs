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
use prod_code_mcp::signature::{Modifiers, Param, change_with};
use prod_code_testkit::Workspace;
use std::path::Path;

pub(super) fn snapshot(root: &Path, file: &str, label: &str) -> String {
    let source = std::fs::read_to_string(root.join(file)).unwrap();
    eprintln!("SNAPSHOT {label} {file}\n{source}END SNAPSHOT");
    run(root, "sha256sum", &[file]);
    source
}

pub(super) fn execute(root: &Path, file: &str) -> String {
    if file.ends_with("py") {
        return run(root, "python3", &[file]);
    }
    if file.ends_with("js") {
        return run(root, "node", &[file]);
    }
    run(
        root,
        "tsc",
        &[
            file,
            "--jsx",
            "react",
            "--allowJs",
            "--noImplicitAny",
            "false",
            "--target",
            "es2022",
            "--module",
            "es2022",
            "--outDir",
            "out",
        ],
    );
    let js = std::fs::read_to_string(root.join("out/main.js")).unwrap();
    std::fs::remove_dir_all(root.join("out")).unwrap();
    run(root, "node", &["--input-type=module", "--eval", &js])
}

#[tokio::test]
async fn python_comparison_effect_is_not_a_keyword() {
    let mut failures = Vec::new();
    for expression in [
        "effect() != 1",
        "effect() == 1",
        "effect() <= 1",
        "effect() >= 1",
        "(value := effect()) == 1",
        "b=effect() != 1",
    ] {
        for force in [false, true] {
            let source = format!(
                "def f(a, b):\n    return a\ndef effect():\n    print('effect')\n    return 0\nprint(f(1, {expression}))\n"
            );
            let ws = Workspace::new(&[("main.py", &source)]);
            let root = &ws.root();
            let label = format!("python-{expression}-{force}");
            snapshot(root, "main.py", &format!("{label}-before"));
            let before = execute(root, "main.py");
            let gw = gateway(vec![location(root, "main.py", &source, "f(1,")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join("main.py"),
                1,
                5,
                &[Param::Keep("a".into())],
                &Modifiers::default(),
                true,
                force,
            )
            .await;
            eprintln!("{label} public result: {result:?}");
            let after_source = snapshot(root, "main.py", &format!("{label}-after"));
            let after = execute(root, "main.py");
            if !result.is_err_and(|e| e.to_string().contains("nothing was written"))
                || after_source != source
                || before != "effect\n1\n"
                || after != before
            {
                failures.push(label);
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
}

#[tokio::test]
async fn callback_only_references_refuse_even_with_force() {
    let mut failures = Vec::new();
    for use_site in [
        "const callback=f; console.log(callback(1,2));",
        "const object={import: f}; console.log(object.import(1,2));",
        "console.log([[1,2]].map(args => args.reduce(f))[0]);",
    ] {
        for force in [false, true] {
            let source = format!("function f(a,b) {{ return a*10+b; }}\n{use_site}\n");
            let ws = Workspace::new(&[("main.js", &source)]);
            let root = &ws.root();
            let label = format!("callback-{force}-{use_site}");
            snapshot(root, "main.js", &format!("{label}-before"));
            let before = execute(root, "main.js");
            let needle = if use_site.contains("callback") {
                "f;"
            } else if use_site.contains("object") {
                "f}"
            } else {
                "f)"
            };
            let gw = gateway(vec![location(root, "main.js", &source, needle)]).await;
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
            let after_source = snapshot(root, "main.js", &format!("{label}-after"));
            let after = execute(root, "main.js");
            if !result.is_err_and(|e| e.to_string().contains("nothing was written"))
                || after_source != source
                || before != "12\n"
                || after != before
            {
                failures.push(label);
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
}

async fn async_fixture(file: &str, expression: &str, enable: bool, label: &str) -> bool {
    let modifier = if enable { "" } else { "async " };
    let source = format!(
        "{modifier}function f(x) {{ return x; }}\nconst React = {{createElement: (...args) => args}};\nasync function main() {{ const view = {expression}; console.log(JSON.stringify(view)); }}\nmain();\n"
    );
    let ws = Workspace::new(&[(file, &source)]);
    let root = &ws.root();
    snapshot(root, file, &format!("{label}-before"));
    let before = execute(root, file);
    let gw = gateway(vec![location(root, file, &source, "f(1)")]).await;
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
    let after_source = snapshot(root, file, &format!("{label}-after"));
    let after = execute(root, file);
    let expected = if enable {
        source
            .replace("function f", "async function f")
            .replace("f(1)", "(await f(1))")
    } else {
        source
            .replace("async function f", "function f")
            .replace("await f(1)", "f(1)")
    };
    result.is_ok() && after_source == expected && before == after && before.contains('1')
}

#[tokio::test]
async fn jsx_import_looking_text_keeps_expression_references() {
    let mut failures = Vec::new();
    for file in ["main.jsx", "main.tsx"] {
        for text in [
            "hello from home",
            "require(f)",
            "import { f } from 'module'",
        ] {
            for enable in [true, false] {
                let awaited = if enable { "" } else { "await " };
                let expression = format!("<div title=\"{text}\">{text}{{{awaited}f(1)}}</div>");
                let label = format!("jsx-import-{file}-{text}-{enable}");
                if !async_fixture(file, &expression, enable, &label).await {
                    failures.push(label);
                }
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
}

#[tokio::test]
async fn jsx_argument_delimiters_remain_markup_text() {
    let mut failures = Vec::new();
    for file in ["main.jsx", "main.tsx"] {
        for argument in [
            "<span>)</span>",
            "<span title=\"),\">),<i>{`)${1}`}</i></span>",
        ] {
            let source = format!(
                "function f(x) {{ return [x, arguments.length, arguments[1] ?? 2]; }}\nconst React = {{createElement: (...args) => args}};\nconsole.log(JSON.stringify(f({argument})));\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            let label = format!("jsx-args-{file}-{argument}");
            snapshot(root, file, &format!("{label}-before"));
            let before = execute(root, file);
            let call = format!("f({argument})");
            let gw = gateway(vec![location(root, file, &source, &call)]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                1,
                10,
                &[
                    Param::Keep("x".into()),
                    Param::Add {
                        name: "extra".into(),
                        ty: String::new(),
                        value: "2".into(),
                    },
                ],
                &Modifiers::default(),
                true,
                false,
            )
            .await;
            eprintln!("{label} public result: {result:?}");
            let actual = snapshot(root, file, &format!("{label}-after"));
            let after = execute(root, file);
            let expected = source
                .replace("function f(x)", "function f(x, extra = 2)")
                .replace(&call, &format!("f({argument}, 2)"));
            if result.is_err() || actual != expected || after != before.replace(",1,2]", ",2,2]") {
                failures.push(label);
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
}

#[tokio::test]
async fn template_substitutions_support_async_both_directions() {
    let mut failures = Vec::new();
    for file in ["main.js", "main.ts", "main.jsx", "main.tsx"] {
        for enable in [true, false] {
            let awaited = if enable { "" } else { "await " };
            let expressions = if file.ends_with("sx") {
                vec![
                    format!("<div>{{`${{{awaited}f(1)}}`}}</div>"),
                    format!("<div>{{`outer ${{<i>{{`inner ${{{awaited}f(1)}}`}}</i>}}`}}</div>"),
                ]
            } else {
                vec![
                    format!("`${{{awaited}f(1)}}`"),
                    format!("`outer ${{`inner ${{{awaited}f(1)}}`}}`"),
                ]
            };
            for expression in expressions {
                let label = format!("template-{file}-{enable}-{expression}");
                if !async_fixture(file, &expression, enable, &label).await {
                    failures.push(label);
                }
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
}
