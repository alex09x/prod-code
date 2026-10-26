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

fn jsx_run(root: &std::path::Path, file: &str) -> String {
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
            "--outDir",
            "out",
        ],
    );
    let output = run(root, "node", &["out/main.js"]);
    // Do not let compiler output supply extra candidates to reference discovery.
    std::fs::remove_dir_all(root.join("out")).unwrap();
    output
}

#[tokio::test]
async fn jsx_elsewhere_preserves_both_async_directions() {
    for file in ["main.jsx", "main.tsx"] {
        for enable in [false, true] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let source = format!(
                "const React = {{createElement: (...args) => args, Fragment: 'fragment'}};\nconst view = <div>hello</div>;\n{modifier}function f(x) {{ return x; }}\nasync function main() {{ console.log({awaited}f( 1 )); }}\nconst after = <span title=\"[\">it's (text)</span>;\nmain();\n"
            );
            let source = if file.ends_with("tsx") {
                source.replacen(";\n", "; const id = <T,>(x: T) => x; const constrained = <T extends {}>(x: T) => x;\n", 1)
            } else {
                source
            };
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            evidence(root, file, "original");
            assert_eq!(jsx_run(root, file), "1\n");
            let gw = gateway(vec![location(root, file, &source, "f( 1 )")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                3,
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
            let generated = std::fs::read_to_string(root.join(file)).unwrap();
            let expected = if enable {
                source
                    .replace("function f", "async function f")
                    .replace("console.log(f( 1 ))", "console.log((await f( 1 )))")
            } else {
                source
                    .replace("async function f", "function f")
                    .replace("await f( 1 )", "f( 1 )")
            };
            assert_eq!(generated, expected);
            assert_eq!(jsx_run(root, file), "1\n");
        }
    }
}

#[tokio::test]
async fn jsx_expressions_and_text_keep_semantic_references() {
    for file in ["main.jsx", "main.tsx"] {
        for enable in [false, true] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let source = format!(
                "const React = {{createElement: (...args) => args, Fragment: 'fragment'}};\nconst view = <><div title=\"[\">it's f(777) {{/[)]/.test(')') ? <i/> : <b/>}}</div></>;\n{modifier}function f(x) {{ return x; }}\nasync function main() {{ const active = <div>{{{awaited}f( 1 )}}</div>; console.log(active[2]); }}\nmain();\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            evidence(root, file, "original");
            assert_eq!(jsx_run(root, file), "1\n");
            let gw = gateway(vec![location(root, file, &source, "f( 1 )")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                3,
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
            let generated = std::fs::read_to_string(root.join(file)).unwrap();
            let expected = if enable {
                source
                    .replace("function f", "async function f")
                    .replace("{f( 1 )}", "{(await f( 1 ))}")
            } else {
                source
                    .replace("async function f", "function f")
                    .replace("await f( 1 )", "f( 1 )")
            };
            assert_eq!(generated, expected);
            assert_eq!(jsx_run(root, file), "1\n");
        }
    }
}

#[tokio::test]
async fn python_grouped_and_explicit_continuations_keep_chaining() {
    for (body, chain) in [("[x]", "[0]"), ("(lambda: x)", "()")] {
        for grouped in [false, true] {
            let value = if grouped {
                format!("(f(1)\n        {chain})")
            } else {
                format!("f(1) \\\n        {chain}")
            };
            let source = format!(
                "import asyncio\ndef f(x):\n    return {body}\nasync def main():\n    value = {value}\n    print(value)\nasyncio.run(main())\n"
            );
            let ws = Workspace::new(&[("main.py", &source)]);
            let root = &ws.root();
            evidence(root, "main.py", "original");
            assert_eq!(run(root, "python3", &["main.py"]), "1\n");
            let gw = gateway(vec![location(root, "main.py", &source, "f(1)")]).await;
            change_with(
                gw.addr(),
                root,
                &root.join("main.py"),
                2,
                5,
                &[Param::Keep("x".into())],
                &Modifiers {
                    asyncness: Some(true),
                    ..Default::default()
                },
                true,
                false,
            )
            .await
            .unwrap();
            evidence(root, "main.py", "generated");
            let generated = std::fs::read_to_string(root.join("main.py")).unwrap();
            assert_eq!(
                generated,
                source
                    .replace("def f", "async def f")
                    .replace("f(1)", "(await f(1))")
            );
            assert_eq!(run(root, "python3", &["main.py"]), "1\n");
        }
    }
}

#[tokio::test]
async fn swift_hex_float_does_not_make_effectful_arguments_pure() {
    for value in ["0x1.8p2 + effect()", "0x1.8p2_effect"] {
        for force in [false, true] {
            let source = format!(
                "func f(a: Double, b: Double) -> Double {{ return a }}\nprint(f(a: 1.0, b: {value}))\n"
            );
            let ws = Workspace::new(&[("main.swift", &source)]);
            let root = &ws.root();
            let gw = gateway(vec![location(root, "main.swift", &source, "f(a: 1.0")]).await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join("main.swift"),
                1,
                6,
                &[Param::Keep("a".into())],
                &Modifiers::default(),
                true,
                force,
            )
            .await;
            assert!(result.unwrap_err().to_string().contains("side effects"));
            assert_eq!(
                std::fs::read_to_string(root.join("main.swift")).unwrap(),
                source
            );
        }
    }
}

#[tokio::test]
async fn python_logical_newline_ends_await_operand() {
    for next in ["[a, b] = [2, 3]", "(a, b) = (2, 3)"] {
        let source = format!(
            "import asyncio\nasync def f(x):\n    return x\nasync def main():\n    x = await f(1) # logical boundary\n    {next}\n    print(x, a, b)\nasyncio.run(main())\n"
        );
        let ws = Workspace::new(&[("main.py", &source)]);
        let root = &ws.root();
        evidence(root, "main.py", "original");
        assert_eq!(run(root, "python3", &["main.py"]), "1 2 3\n");
        let gw = gateway(vec![location(root, "main.py", &source, "f(1)")]).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.py"),
            2,
            11,
            &[Param::Keep("x".into())],
            &Modifiers {
                asyncness: Some(false),
                ..Default::default()
            },
            true,
            false,
        )
        .await;
        eprintln!("public result: {result:?}");
        evidence(root, "main.py", "generated");
        result.unwrap();
        let generated = std::fs::read_to_string(root.join("main.py")).unwrap();
        assert_eq!(run(root, "python3", &["main.py"]), "1 2 3\n");
        assert_eq!(
            generated,
            source
                .replace("async def f", "def f")
                .replace("await f(1)", "f(1)")
        );
    }
}

#[tokio::test]
async fn swift_hex_float_remains_reorderable() {
    for literal in ["0x1.8p2", "0x1p-1", "0x1_0.8_0p+0_2"] {
        let source = format!(
            "func f(a: Double, b: Double) -> Double {{ return a * 10 + b }}\nprint(f(a: {literal}, b: 2.0))\n"
        );
        let ws = Workspace::new(&[("main.swift", &source)]);
        let root = &ws.root();
        evidence(root, "main.swift", "original");
        let swift_available = std::process::Command::new("swift")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        let original = swift_available.then(|| run(root, "swift", &["main.swift"]));
        if !swift_available {
            eprintln!(
                "LIMITATION: Swift compiler unavailable; transformation assertions remain required"
            );
        }
        let gw = gateway(vec![location(
            root,
            "main.swift",
            &source,
            &format!("f(a: {literal}"),
        )])
        .await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.swift"),
            1,
            6,
            &[Param::Keep("b".into()), Param::Keep("a".into())],
            &Modifiers::default(),
            true,
            false,
        )
        .await;
        eprintln!("public result: {result:?}");
        evidence(root, "main.swift", "generated");
        result.unwrap();
        let generated = std::fs::read_to_string(root.join("main.swift")).unwrap();
        assert_eq!(
            generated,
            source
                .replace("f(a: Double, b: Double)", "f(b: Double, a: Double)")
                .replace(
                    &format!("f(a: {literal}, b: 2.0)"),
                    &format!("f(b: 2.0, a: {literal})")
                )
        );
        if let Some(original) = original {
            assert_eq!(run(root, "swift", &["main.swift"]), original);
        }
    }
}
