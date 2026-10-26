/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    c_context_controls::{compile_run, node},
    support::*,
};
use serde_json::json;

#[tokio::test]
async fn c_cast_controls_preserve_nested_constant_contexts() {
    for (
        body,
        cast_text,
        ty_text,
        operand_text,
        constant_text,
        nested_text,
        kind,
        count,
        expected,
    ) in [
        (
            "int a=80; auto p=static_cast<int(*)[true ? 80 : 1]>(nullptr); return a+sizeof(*p)/sizeof(int);",
            "static_cast<int(*)[true ? 80 : 1]>(nullptr)",
            "int(*)[true ? 80 : 1]",
            "nullptr",
            "true ? 80 : 1",
            "int(*)[true ? 80 : 1]",
            "type",
            1,
            "90\n",
        ),
        (
            "int a=80; return a+static_cast<int>(constant<true ? 80 : 1>()+80);",
            "static_cast<int>(constant<true ? 80 : 1>()+80)",
            "<int>",
            "constant<true ? 80 : 1>()+80",
            "true ? 80 : 1",
            "constant<true ? 80 : 1>",
            "expression",
            2,
            "100\n",
        ),
    ] {
        let source = format!(
            "#include <stdio.h>\ntemplate<int N> int constant() {{return N;}}\nint target(void) {{\n{body}\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        let original = compile_run(&source, "cpp").unwrap();
        let mut cast = node(&source, cast_text, "expression", "CXXStaticCast");
        let mut ty = node(
            &source,
            ty_text,
            "type",
            if kind == "type" { "Pointer" } else { "Builtin" },
        );
        if ty_text == "<int>" {
            ty["range"]["start"]["character"] =
                json!(ty["range"]["start"]["character"].as_u64().unwrap() + 1);
            ty["range"]["end"]["character"] =
                json!(ty["range"]["end"]["character"].as_u64().unwrap() - 1);
        }
        let mut operand = node(
            &source,
            operand_text,
            "expression",
            if kind == "type" {
                "ImplicitCast"
            } else {
                "BinaryOperator"
            },
        );
        let mut nested = node(
            &source,
            nested_text,
            kind,
            if kind == "type" {
                "ConstantArray"
            } else {
                "DeclRef"
            },
        );
        if kind == "expression" {
            nested["children"] = json!([node(
                &source,
                constant_text,
                "template argument",
                "Expression"
            )]);
            operand["children"] = json!([nested.clone()]);
        }
        cast["children"] = json!([ty, operand]);
        let nested_at = if kind == "type" {
            source.find("[true").unwrap()
        } else {
            source.find("<true").unwrap()
        };
        let asts = [
            (
                source.find("static_cast<").unwrap() + "static_cast".len(),
                cast,
            ),
            (nested_at, nested),
        ];
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, "cpp");
            let plan = change_with_ast(
                &ws,
                &source,
                "cpp",
                "80;",
                true,
                apply,
                force,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await
            .unwrap();
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target(80)", "target(10)");
            println!(
                "CAST CONSTANT {kind} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM\n{custom}",
                plan.replaced
            );
            assert_eq!(plan.replaced, count);
            assert!(generated.contains(constant_text));
            assert_eq!(
                compile_run(generated, "cpp").as_deref(),
                Some(original.as_str())
            );
            assert_eq!(compile_run(&custom, "cpp").as_deref(), Some(expected));
            let ws = workspace(&source, "cpp");
            let error = change_with_ast(
                &ws,
                &source,
                "cpp",
                "80 :",
                false,
                apply,
                force,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await
            .unwrap_err();
            println!("REFUSED cast constant: {error}");
            assert_eq!(ws.read("main.cpp"), source);
        }
    }
}

#[tokio::test]
async fn c_cast_controls_unknown_context_refuses_before_writes() {
    for (ext, source, marker, base) in [
        (
            "c",
            "int target(void) {\nint a=80; int v[81]={[1 ? 80 : 0]=7}; return a+v[80];\n}\nint main(void) {return target();}\n",
            "[1 ?",
            "[1 ? 80 : 0]=7",
        ),
        (
            "cpp",
            "int target(void) {\nint a=80; return a+static_cast<int>(80);\n}\nint main(void) {return target();}\n",
            "<int>",
            "static_cast<int>(80)",
        ),
    ] {
        let kind = if ext == "c" {
            "DesignatedInit"
        } else {
            "CXXStaticCast"
        };
        let valid_range = node(source, base, "expression", kind);
        for ast in [
            serde_json::Value::Null,
            json!({"role":"expression","kind":kind}),
            valid_range,
        ] {
            for force in [false, true] {
                let ws = workspace(source, ext);
                let error = change_with_ast(
                    &ws,
                    source,
                    ext,
                    "80;",
                    true,
                    true,
                    force,
                    false,
                    "int",
                    &[],
                    &[],
                    &[(source.find(marker).unwrap(), ast.clone())],
                )
                .await
                .unwrap_err();
                println!("REFUSED unknown {ext} force={force}: {error}");
                assert_eq!(ws.read(&format!("main.{ext}")), source);
            }
        }
    }
}
