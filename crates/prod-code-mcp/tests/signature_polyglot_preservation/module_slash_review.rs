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
async fn unrelated_slashes_preserve_ordinary_signature_and_async_calls() {
    let mut failures = Vec::new();
    for (file, unrelated) in [
        ("main.js", "{ const unused = 0; } /[)]/.test(')');"),
        ("main.js", "{} {} /[)]/.test(')');"),
        ("main.js", "{ {} /[)]/.test(')'); }"),
        (
            "main.js",
            "function other() { {} /[)]/.test(')'); } other();",
        ),
        ("main.ts", "{ const unused = 0; } /[)]/.test(')');"),
        ("main.jsx", "{ const unused = 0; } /[)]/.test(')');"),
        ("main.tsx", "{ const unused = 0; } /[)]/.test(')');"),
        ("main.js", "const unused = {} / /[)]/.source.length;"),
        ("main.js", "const unused = `${ {} / /[)]/.source.length }`;"),
        ("main.js", "const unused = `${0}${{} / 2}`;"),
        (
            "main.js",
            "const unused = `${0}${{} / /[)]/.source.length}`;",
        ),
        (
            "main.jsx",
            "const React = { createElement: (...args) => args }; const unused = <div>{0}{{} / 2}</div>;",
        ),
        (
            "main.jsx",
            "const React = { createElement: (...args) => args }; const unused = <div>{{} / /[)]/.source.length}</div>;",
        ),
        (
            "main.js",
            "const object = {return: 4}; const n = object.return / 2;",
        ),
        (
            "main.js",
            "const object = {throw: 4}; const n = object.throw / 2;",
        ),
        (
            "main.ts",
            "const x: number | undefined = 4; const n = x! / 2;",
        ),
        ("main.js", "class C {} /[)]/.test(')');"),
        ("main.js", "try {} catch {} /[)]/.test(')');"),
        (
            "main.ts",
            "try { throw 0; } catch /* optional binding */ {} /[)]/.test(')');",
        ),
        ("main.jsx", "try {} catch {} /[)]/.test(')');"),
        ("main.tsx", "try {} catch {} /[)]/.test(')');"),
        (
            "main.js",
            "class Base {} class C extends Base {} /[)]/.test(')');",
        ),
        ("main.js", "const x = 1\nclass C {} /[)]/.test(')');"),
        ("main.js", "const n = class C {} / 2;"),
        (
            "main.js",
            "const C = class {}\nfunction other() {} /[)]/.test(')');",
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
            let label = format!("slash-{unrelated}-{asyncness:?}");
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

#[tokio::test]
async fn type_only_reexports_preserve_signature_and_async_operations() {
    let mut failures = Vec::new();
    for reexport in [
        "export type { f } from './lib';\n",
        "export type /* binding */ { f as renamed } from './lib';\n",
        "export { f } from './lib';\n",
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
            let library = format!("export {modifier}function f(a,b) {{ return a*10+b; }}\n");
            let source = format!(
                "import {{ f }} from './lib';\nasync function main() {{ console.log({awaited}f(1,2)); }}\nmain();\n"
            );
            let ws = Workspace::new(&[
                ("main.ts", &source),
                ("lib.ts", &library),
                ("barrel.ts", reexport),
            ]);
            let root = &ws.root();
            let label = format!("reexport-{reexport:?}-{asyncness:?}");
            for file in ["main.ts", "lib.ts", "barrel.ts"] {
                snapshot(root, file, &format!("{label}-before"));
            }
            let run_program = || {
                run(
                    root,
                    "tsc",
                    &[
                        "main.ts",
                        "barrel.ts",
                        "--noImplicitAny",
                        "false",
                        "--target",
                        "es2022",
                        "--module",
                        "commonjs",
                        "--outDir",
                        "out",
                    ],
                );
                let output = run(root, "node", &["out/main.js"]);
                std::fs::remove_dir_all(root.join("out")).unwrap();
                output
            };
            assert_eq!(run_program(), "12\n");
            let gw = gateway(vec![
                location(root, "main.ts", &source, "f }"),
                location(root, "main.ts", &source, "f(1,2)"),
                location(root, "barrel.ts", reexport, "f "),
            ])
            .await;
            let params = if asyncness.is_none() {
                vec![Param::Keep("b".into()), Param::Keep("a".into())]
            } else {
                vec![Param::Keep("a".into()), Param::Keep("b".into())]
            };
            let result = change_with(
                gw.addr(),
                root,
                &root.join("lib.ts"),
                1,
                17,
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
            let actual = snapshot(root, "main.ts", &format!("{label}-after"));
            let actual_library = snapshot(root, "lib.ts", &format!("{label}-after"));
            assert_eq!(
                snapshot(root, "barrel.ts", &format!("{label}-after")),
                reexport
            );
            assert_eq!(run_program(), "12\n");
            let (expected, expected_library) = match asyncness {
                None => (
                    source.replace("f(1,2)", "f(2, 1)"),
                    library.replace("f(a,b)", "f(b, a)"),
                ),
                Some(true) => (
                    source.replace("f(1,2)", "(await f(1,2))"),
                    library
                        .replace("function f", "async function f")
                        .replace("f(a,b)", "f(a, b)"),
                ),
                Some(false) => (
                    source.replace("await f(1,2)", "f(1,2)"),
                    library
                        .replace("async function f", "function f")
                        .replace("f(a,b)", "f(a, b)"),
                ),
            };
            if result.is_err() || actual != expected || actual_library != expected_library {
                failures.push(format!("{label}: actual={actual:?}, library={actual_library:?}, expected={expected:?}, expected_library={expected_library:?}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
