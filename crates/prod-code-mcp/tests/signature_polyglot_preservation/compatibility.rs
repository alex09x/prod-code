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

#[tokio::test]
async fn default_export_regex_preserves_async_changes() {
    for enable in [false, true] {
        let modifier = if enable { "" } else { "async " };
        let awaited = if enable { "" } else { "await " };
        let source = format!(
            "export default /[)]/;\n{modifier}function f(x) {{ return x; }}\nasync function main() {{ console.log({awaited}f( 1 )); }}\nmain();\n"
        );
        let ws = Workspace::new(&[
            ("main.js", &source),
            ("package.json", "{\"type\":\"module\"}\n"),
        ]);
        let root = &ws.root();
        evidence(root, "main.js", "original");
        assert_eq!(run(root, "node", &["main.js"]), "1\n");
        let gw = gateway(vec![location(root, "main.js", &source, "f( 1 )")]).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.js"),
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
        evidence(root, "main.js", "generated");
        assert_eq!(run(root, "node", &["main.js"]), "1\n");
        result.unwrap();
        let generated = std::fs::read_to_string(root.join("main.js")).unwrap();
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
    }
}

#[tokio::test]
async fn dollar_identifiers_preserve_both_async_directions() {
    for file in ["main.js", "main.ts"] {
        for (method, enable) in [(false, false), (false, true), (true, false)] {
            let modifier = if enable { "" } else { "async " };
            let awaited = if enable { "" } else { "await " };
            let declaration = if method {
                format!(
                    "class C {{\n    {modifier}$fetch(x) {{ return x; }}\n}}\nconst c = new C();\n"
                )
            } else {
                format!("{modifier}function $fetch(x) {{ return x; }}\n")
            };
            let receiver = if method { "c." } else { "" };
            let source = format!(
                "{declaration}async function outer(x) {{ return x; }}\nasync function main() {{\n    console.log(await outer({awaited}(({receiver}$fetch( 1 )))));\n}}\nmain();\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            evidence(root, file, "original");
            // The TypeScript fixture is a JavaScript subset; Node executes
            // those exact bytes, without pretending to typecheck TypeScript.
            assert_eq!(run(root, "node", &["-e", &source]), "1\n");
            let gw = gateway(vec![location(root, file, &source, "$fetch( 1 )")]).await;
            let declaration_at = source.find("$fetch(x)").unwrap();
            let line = if method { 2 } else { 1 };
            let column = source[..declaration_at].rsplit('\n').next().unwrap().len() as u32 + 1;
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                line,
                column,
                &[Param::Keep("x".into())],
                &Modifiers {
                    asyncness: Some(enable),
                    ..Default::default()
                },
                true,
                false,
            )
            .await;
            evidence(root, file, "generated");
            let generated = std::fs::read_to_string(root.join(file)).unwrap();
            assert_eq!(run(root, "node", &["-e", &generated]), "1\n");
            result.unwrap();
            // Check the complete source, including the outer callee's await
            // and the exact spaces inside the unchanged argument list.
            let expected = if enable {
                let declaration = if method {
                    "$fetch(x)"
                } else {
                    "function $fetch(x)"
                };
                source
                    .replacen(declaration, &format!("async {declaration}"), 1)
                    .replace(
                        &format!("{receiver}$fetch( 1 )"),
                        &format!("(await {receiver}$fetch( 1 ))"),
                    )
            } else {
                source
                    .replacen("async ", "", 1)
                    .replace("outer(await ", "outer(")
            };
            assert_eq!(generated, expected);
        }
    }
}

#[tokio::test]
async fn hexadecimal_user_defined_suffix_is_not_scalar_proof() {
    let source = "#include <cstdio>\nlong double operator \"\" _effect(long double x) { puts(\"effect\"); return x; }\ndouble f(double a, double b) { return a; }\nint main() { printf(\"%.1f\\n\", f(1.0, 0x1.8p+2_effect)); }\n";
    for force in [false, true] {
        let ws = Workspace::new(&[("main.cpp", source)]);
        let root = &ws.root();
        evidence(root, "main.cpp", "original");
        run(root, "c++", &["-std=c++17", "main.cpp", "-o", "program"]);
        assert_eq!(run(root, "./program", &[]), "effect\n1.0\n");
        let gw = gateway(vec![location(
            root,
            "main.cpp",
            source,
            "f(1.0, 0x1.8p+2_effect)",
        )])
        .await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.cpp"),
            3,
            8,
            &[Param::Keep("a".into())],
            &Modifiers::default(),
            true,
            force,
        )
        .await;
        evidence(root, "main.cpp", "generated");
        assert_eq!(
            std::fs::read_to_string(root.join("main.cpp")).unwrap(),
            source
        );
        assert!(result.unwrap_err().to_string().contains("side effects"));
        run(root, "c++", &["-std=c++17", "main.cpp", "-o", "program"]);
        assert_eq!(run(root, "./program", &[]), "effect\n1.0\n");
    }
}

#[tokio::test]
async fn builtin_hexadecimal_floats_remain_reorderable() {
    for (file, compiler, standard) in [
        ("main.cpp", "c++", "-std=c++17"),
        ("main.c", "cc", "-std=c11"),
    ] {
        for literal in ["0x1.8p+2", "0X1P-1", "0x.8p2f", "-0x1.p0L"] {
            let source = format!(
                "#include <stdio.h>\ndouble f(double a, double b) {{ return a * 10 + b; }}\nint main() {{ printf(\"%.1f\\n\", f({literal}, 2.0)); }}\n"
            );
            let ws = Workspace::new(&[(file, &source)]);
            let root = &ws.root();
            evidence(root, file, "original");
            run(root, compiler, &[standard, file, "-o", "program"]);
            let original = run(root, "./program", &[]);
            let gw = gateway(vec![location(
                root,
                file,
                &source,
                &format!("f({literal}, 2.0)"),
            )])
            .await;
            let result = change_with(
                gw.addr(),
                root,
                &root.join(file),
                2,
                8,
                &[Param::Keep("b".into()), Param::Keep("a".into())],
                &Modifiers::default(),
                true,
                false,
            )
            .await;
            evidence(root, file, "generated");
            run(root, compiler, &[standard, file, "-o", "program"]);
            assert_eq!(run(root, "./program", &[]), original);
            result.unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join(file)).unwrap(),
                source
                    .replace("f(double a, double b)", "f(double b, double a)")
                    .replace(&format!("f({literal}, 2.0)"), &format!("f(2.0, {literal})"))
            );
        }
    }
}
