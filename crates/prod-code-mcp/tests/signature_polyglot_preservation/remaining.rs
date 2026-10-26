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
async fn unrelated_javascript_regex_does_not_block_async_changes() {
    for enable in [false, true] {
        let modifier = if enable { "" } else { "async " };
        let await_keyword = if enable { "" } else { "await " };
        let source = format!(
            "const before = /[)]/;\n{modifier}function f(x) {{ return x; }}\nasync function main() {{\n    let i = 4; const quotient = i++ / 2; const object = {{}} / 2;\n    if (i) /[)\\/]/.test(')');\n    console.log({await_keyword}f( 1 ));\n}}\nconst after = /[(]/;\nmain();\n"
        );
        let ws = Workspace::new(&[("main.js", &source)]);
        let root = &ws.root();
        evidence(root, "main.js", "original");
        let original = run(root, "node", &["main.js"]);
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
        assert_eq!(run(root, "node", &["main.js"]), original);
        result.unwrap();
        let generated = std::fs::read_to_string(root.join("main.js")).unwrap();
        assert!(generated.contains("f( 1 )"));
        assert!(generated.contains("const before = /[)]/;"));
        assert!(generated.contains("const after = /[(]/;"));
    }
}

#[tokio::test]
async fn ordinary_javascript_numeric_literals_remain_reorderable() {
    for literal in [
        "0x10", "0XFF", "0b1010", "0o17", "1_000", "1.2_5e+2", "0x10n",
    ] {
        let source = format!(
            "function f(a,b) {{ return [String(a),String(b)]; }}\nconsole.log(JSON.stringify(f({literal}, 2)));\n"
        );
        reorder_numeric("main.js", &source, "node", 10, &format!("f({literal}, 2)")).await;
    }
}

#[tokio::test]
async fn ordinary_python_numeric_literals_remain_reorderable() {
    for literal in ["1_000", "0x10", "0b1010", "0o17", "1.2_5e+2", "0x_FF"] {
        let source = format!("def f(a,b):\n    return [a,b]\nprint(f({literal}, 2))\n");
        reorder_numeric(
            "main.py",
            &source,
            "python3",
            5,
            &format!("f({literal}, 2)"),
        )
        .await;
    }
}

#[tokio::test]
async fn javascript_throwing_bigint_conversion_is_not_pure() {
    let source = "function f(a,b) { return a; }\ntry { console.log(f(1, +1n)); } catch (e) { console.log(e.name); }\n";
    for force in [false, true] {
        let ws = Workspace::new(&[("main.js", source)]);
        let root = &ws.root();
        let original = run(root, "node", &["main.js"]);
        assert_eq!(original, "TypeError\n");
        let gw = gateway(vec![location(root, "main.js", source, "f(1, +1n)")]).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.js"),
            1,
            10,
            &[Param::Keep("a".into())],
            &Modifiers::default(),
            true,
            force,
        )
        .await;
        assert_eq!(run(root, "node", &["main.js"]), original);
        assert!(result.unwrap_err().to_string().contains("side effects"));
        assert_eq!(
            std::fs::read_to_string(root.join("main.js")).unwrap(),
            source
        );
    }
}

async fn reorder_numeric(file: &str, source: &str, runtime: &str, column: u32, call: &str) {
    let ws = Workspace::new(&[(file, source)]);
    let root = &ws.root();
    evidence(root, file, "original");
    let original = run(root, runtime, &[file]);
    let gw = gateway(vec![location(root, file, source, call)]).await;
    let result = change_with(
        gw.addr(),
        root,
        &root.join(file),
        1,
        column,
        &[Param::Keep("b".into()), Param::Keep("a".into())],
        &Modifiers::default(),
        true,
        false,
    )
    .await;
    evidence(root, file, "generated");
    assert_eq!(run(root, runtime, &[file]), original);
    result.unwrap();
    let generated = std::fs::read_to_string(root.join(file)).unwrap();
    assert!(generated.contains("f(b, a)"));
}

#[tokio::test]
async fn cpp_numeric_proof_distinguishes_literals_from_user_defined_operators() {
    for literal in ["0x10", "1000", "1UL", "1.0f", "0b11", "1_000", "1f"] {
        let source = format!(
            "#include <cstdio>\nint operator \"\" _000(unsigned long long) {{ puts(\"effect\"); return 9; }}\nint operator \"\" f(unsigned long long) {{ puts(\"effect\"); return 9; }}\nint f(int a, int b) {{ return a; }}\nint main() {{ printf(\"%d\\n\", f(1, {literal})); }}\n"
        );
        let ws = Workspace::new(&[("main.cpp", &source)]);
        let root = &ws.root();
        evidence(root, "main.cpp", "original");
        run(root, "c++", &["main.cpp", "-o", "program"]);
        let original = run(root, "./program", &[]);
        let gw = gateway(vec![location(
            root,
            "main.cpp",
            &source,
            &format!("f(1, {literal})"),
        )])
        .await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("main.cpp"),
            4,
            5,
            &[Param::Keep("a".into())],
            &Modifiers::default(),
            true,
            true,
        )
        .await;
        evidence(root, "main.cpp", "generated");
        run(root, "c++", &["main.cpp", "-o", "program"]);
        assert_eq!(run(root, "./program", &[]), original);
        if matches!(literal, "1_000" | "1f") {
            assert!(result.unwrap_err().to_string().contains("side effects"));
            assert_eq!(
                std::fs::read_to_string(root.join("main.cpp")).unwrap(),
                source
            );
        } else {
            result.unwrap();
            assert!(
                std::fs::read_to_string(root.join("main.cpp"))
                    .unwrap()
                    .contains("f(1)")
            );
        }
    }
}

#[tokio::test]
async fn selected_header_checks_removed_definition_parameter_with_local_name() {
    for force in [false, true] {
        let header = "int f(int left, int right);\n";
        let source = "#include \"api.h\"\n#include <cstdio>\nint b=100;\nint f(int a, int b) { return a+b; }\nint main() { printf(\"%d\\n\", f(1, 2)); }\n";
        let ws = Workspace::new(&[("api.h", header), ("main.cpp", source)]);
        let root = &ws.root();
        evidence(root, "api.h", "original");
        evidence(root, "main.cpp", "original");
        run(root, "c++", &["main.cpp", "-o", "program"]);
        assert_eq!(run(root, "./program", &[]), "3\n");
        let (refs, declarations) = super::semantic::cpp_references(root, source).await;
        for expected in [
            location(root, "main.cpp", source, "f(int a, int b)"),
            location(root, "main.cpp", source, "f(1, 2)"),
            location(root, "api.h", header, "f(int left, int right)"),
        ] {
            assert!(
                refs.iter()
                    .chain(&declarations)
                    .any(|r| r["uri"] == expected["uri"]
                        && r["range"]["start"] == expected["range"]["start"])
            );
        }
        let gw = gateway_with_declarations(refs, declarations).await;
        let result = change_with(
            gw.addr(),
            root,
            &root.join("api.h"),
            1,
            5,
            &[Param::Keep("left".into())],
            &Modifiers::default(),
            true,
            force,
        )
        .await;
        evidence(root, "api.h", "generated");
        evidence(root, "main.cpp", "generated");
        run(root, "c++", &["main.cpp", "-o", "program"]);
        let output = run(root, "./program", &[]);
        if force {
            result.unwrap();
            assert_eq!(
                output, "101\n",
                "force explicitly overrides body-use protection"
            );
        } else {
            assert_eq!(
                output, "3\n",
                "unforced change silently rebound the local name"
            );
            let message = result.unwrap_err().to_string();
            assert!(message.contains("nothing was written") && message.contains("still used"));
            assert_eq!(std::fs::read_to_string(root.join("api.h")).unwrap(), header);
            assert_eq!(
                std::fs::read_to_string(root.join("main.cpp")).unwrap(),
                source
            );
        }
    }
}
