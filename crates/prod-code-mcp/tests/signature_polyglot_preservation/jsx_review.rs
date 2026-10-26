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

fn execute(root: &Path, file: &str) -> String {
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
    // Compiler output must not become an extra source candidate for discovery.
    std::fs::remove_dir_all(root.join("out")).unwrap();
    run(root, "node", &["--input-type=module", "--eval", &js])
}

#[tokio::test]
async fn tsx_generic_arrow_before_top_level_call() {
    for arrow in [
        "<T,>(x: T) => x",
        "<T extends {}>(x: T) => x",
        "<const T,>(x: T) => x",
        "<T = number,>(x: T): T => x",
        "<T /* parameter */ ,>(x: T) => x",
        "<T extends Array<number>>(x: T): T => x",
        "<T extends { value?: (a: number) => number }>(x: T): T => x",
    ] {
        for enable in [false, true] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let source = format!(
                "export {{}};\n{modifier}function f(x: number) {{ return x; }}\nconst id = {arrow};\nconst value = {awaited}f(1);\nconsole.log(value);\n"
            );
            let ws = Workspace::new(&[("main.tsx", &source)]);
            let root = &ws.root();
            evidence(root, "main.tsx", "original");
            assert_eq!(execute(root, "main.tsx"), "1\n");
            let gw = gateway(vec![location(root, "main.tsx", &source, "f(1)")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join("main.tsx"),
                2,
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
            eprintln!("public result: {result:?}");
            evidence(root, "main.tsx", "generated");
            result.unwrap();
            let expected = if enable {
                source
                    .replace("function f", "async function f")
                    .replace("value = f(1)", "value = (await f(1))")
            } else {
                source
                    .replace("async function f", "function f")
                    .replace("await f(1)", "f(1)")
            };
            assert_eq!(
                std::fs::read_to_string(root.join("main.tsx")).unwrap(),
                expected
            );
            assert_eq!(execute(root, "main.tsx"), "1\n");
        }
    }
}

#[tokio::test]
async fn tsx_generic_function_type_enable_async() {
    generic_function_type_async(true).await;
}

#[tokio::test]
async fn tsx_generic_function_type_disable_async() {
    generic_function_type_async(false).await;
}

async fn generic_function_type_async(enable: bool) {
    for declaration in [
        "type Fn = <T>(x: T) => T;",
        "type Fn = <T /* parameter */>(x: T) => T;",
        "type Fn = { call: <T>(x: T) => T };",
        "type Fn = <T>(x: T) => <U>(y: U) => T;",
        "type Fn = new <T>(x: T) => { value: T };",
    ] {
        let modifier = if enable { "" } else { "async " };
        let awaited = if enable { "" } else { "await " };
        let source = format!(
            "export {{}};\n{modifier}function f(x: number) {{ return x; }}\n{declaration}\nconst value = {awaited}f(1);\nconsole.log(value);\n"
        );
        let ws = Workspace::new(&[("main.tsx", &source)]);
        let root = &ws.root();
        evidence(root, "main.tsx", "original");
        assert_eq!(execute(root, "main.tsx"), "1\n");
        let gw = gateway(vec![location(root, "main.tsx", &source, "f(1)")]).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.tsx"),
            2,
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
        eprintln!("public result: {result:?}");
        evidence(root, "main.tsx", "generated");
        result.unwrap();
        let expected = if enable {
            source
                .replace("function f", "async function f")
                .replace("value = f(1)", "value = (await f(1))")
        } else {
            source
                .replace("async function f", "function f")
                .replace("await f(1)", "f(1)")
        };
        assert_eq!(
            std::fs::read_to_string(root.join("main.tsx")).unwrap(),
            expected
        );
        assert_eq!(execute(root, "main.tsx"), "1\n");
    }
}

#[tokio::test]
async fn jsx_apostrophe_before_only_semantic_call() {
    for file in ["main.jsx", "main.tsx"] {
        for enable in [true, false] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let source = format!(
                "{modifier}function f(x) {{ return x; }}\nconst React = {{createElement: (...args) => args}};\nconst view = <div>it's text</div>;\nasync function main() {{ console.log({awaited}f(1)); }}\nmain();\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            evidence(root, file, "original");
            assert_eq!(execute(root, file), "1\n");
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
            eprintln!("public result: {result:?}");
            evidence(root, file, "generated");
            result.unwrap();
            let actual_output = execute(root, file);
            eprintln!("generated runtime: {actual_output:?}");
            let expected = if enable {
                source
                    .replace("function f", "async function f")
                    .replace("console.log(f(1))", "console.log((await f(1)))")
            } else {
                source
                    .replace("async function f", "function f")
                    .replace("await f(1)", "f(1)")
            };
            assert_eq!(std::fs::read_to_string(root.join(file)).unwrap(), expected);
            assert_eq!(actual_output, "1\n");
        }
    }
}

#[tokio::test]
async fn jsx_extends_attribute_is_not_a_generic_arrow() {
    let source = "function f(x) { return x; }\nconst React = {createElement: (...args) => args};\nconst view = <div extends=\"yes\" other>(text): text</div>;\nasync function main() { console.log(f(1)); }\nmain();\n";
    let ws = Workspace::new(&[("main.tsx", source)]);
    let root = &ws.root();
    evidence(root, "main.tsx", "original");
    assert_eq!(execute(root, "main.tsx"), "1\n");
    let gw = gateway(vec![location(root, "main.tsx", source, "f(1)")]).await;
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
    eprintln!("public result: {result:?}");
    evidence(root, "main.tsx", "generated");
    result.unwrap();
    let expected = source
        .replace("function f", "async function f")
        .replace("console.log(f(1))", "console.log((await f(1)))");
    assert_eq!(
        std::fs::read_to_string(root.join("main.tsx")).unwrap(),
        expected
    );
    assert_eq!(execute(root, "main.tsx"), "1\n");
}
