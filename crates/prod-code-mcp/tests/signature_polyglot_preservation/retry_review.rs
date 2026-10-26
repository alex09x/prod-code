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
async fn complete_tagged_template_receivers_support_async_changes() {
    for receiver in [
        "getC`text`",
        "getC``",
        "getC`a${`b${1}`}c`",
        "(getC`text`)",
        "getTag`one``two`",
    ] {
        for enable in [true, false] {
            for force in [false, true] {
                let awaited = if enable { "" } else { "await " };
                let source = format!(
                    "class C {{\n  async f(x) {{ return x; }}\n}}\nfunction getC(_) {{ return new C(); }}\nfunction getTag(_) {{ return getC; }}\nasync function main() {{ return {awaited}{receiver}.f(1); }}\nmain().then(console.log);\n"
                );
                let ws = Workspace::new(&[("main.js", &source)]);
                let root = &ws.root();
                let label = format!("tagged-{receiver}-{enable}-{force}");
                snapshot(root, "main.js", &format!("{label}-before"));
                assert_eq!(execute(root, "main.js"), "1\n");
                let gw = gateway(vec![location(root, "main.js", &source, "f(1)")]).await;
                let result = change_with(
                    gw.addr(),
                    root,
                    &root.join("main.js"),
                    2,
                    3,
                    &[Param::Keep("x".into())],
                    &Modifiers {
                        asyncness: Some(enable),
                        ..Default::default()
                    },
                    true,
                    force,
                )
                .await;
                eprintln!("{label} public result: {result:?}");
                let actual = snapshot(root, "main.js", &format!("{label}-after"));
                assert_eq!(execute(root, "main.js"), "1\n");
                result.unwrap();
                let expected = if enable {
                    source.clone().replace(
                        &format!("{receiver}.f(1)"),
                        &format!("(await {receiver}.f(1))"),
                    )
                } else {
                    source.replace("  async f(x)", "  f(x)").replace(
                        &format!("await {receiver}.f(1)"),
                        &format!("{receiver}.f(1)"),
                    )
                };
                assert_eq!(actual, expected);
            }
        }
    }
}

#[tokio::test]
async fn import_equals_entity_boundary_preserves_following_calls() {
    for alias in [
        "import Alias = N.C\n",
        "import Alias = N.C; ",
        "import Alias = N\n . C\n",
    ] {
        for enable in [true, false] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let source = format!(
                "namespace N {{ export class C {{}} }}\n{modifier}function f(x) {{ return x; }}\n{alias}async function main() {{ console.log({awaited}f(1)); }}\nmain();\n"
            );
            let ws = Workspace::new(&[("main.ts", &source)]);
            let root = &ws.root();
            let label = format!("import-equals-{alias:?}-{enable}");
            snapshot(root, "main.ts", &format!("{label}-before"));
            assert_eq!(execute(root, "main.ts"), "1\n");
            let gw = gateway(vec![location(root, "main.ts", &source, "f(1)")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join("main.ts"),
                2,
                10,
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
            let actual = snapshot(root, "main.ts", &format!("{label}-after"));
            assert_eq!(execute(root, "main.ts"), "1\n");
            result.unwrap();
            let expected = if enable {
                source
                    .replace("function f", "async function f")
                    .replace("f(1)", "(await f(1))")
            } else {
                source
                    .replace("async function f", "function f")
                    .replace("await f(1)", "f(1)")
            };
            assert_eq!(actual, expected);
        }
    }
}

#[tokio::test]
async fn typescript_assertions_keep_import_binding_classification() {
    for enable in [true, false] {
        let modifier = if enable { "" } else { "async " };
        let awaited = if enable { "" } else { "await " };
        let library = format!("export {modifier}function f(x) {{ return x; }}\n");
        let source = format!(
            "import {{ f }} from './lib';\nconst n = (<number>1);\nasync function main() {{ console.log({awaited}f(n)); }}\nmain();\n"
        );
        let ws = Workspace::new(&[("main.ts", &source), ("lib.ts", &library)]);
        let root = &ws.root();
        let label = format!("ts-assertion-{enable}");
        snapshot(root, "main.ts", &format!("{label}-before"));
        snapshot(root, "lib.ts", &format!("{label}-before"));
        let run_program = || {
            run(
                root,
                "tsc",
                &[
                    "main.ts",
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
        assert_eq!(run_program(), "1\n");
        let gw = gateway(vec![
            location(root, "main.ts", &source, "f }"),
            location(root, "main.ts", &source, "f(n)"),
        ])
        .await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("lib.ts"),
            1,
            17,
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
        let actual = snapshot(root, "main.ts", &format!("{label}-after"));
        let actual_library = snapshot(root, "lib.ts", &format!("{label}-after"));
        assert_eq!(run_program(), "1\n");
        result.unwrap();
        assert_eq!(
            actual,
            if enable {
                source.replace("f(n)", "(await f(n))")
            } else {
                source.replace("await f(n)", "f(n)")
            }
        );
        assert_eq!(
            actual_library,
            if enable {
                library.replace("function f", "async function f")
            } else {
                library.replace("async function f", "function f")
            }
        );
    }
}
