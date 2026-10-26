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
use serde_json::{Value, json};

// These semantic replies are scripted. The separate probe records live clangd;
// compile_run executes real strict C11/C++17 compilers and programs on the node.
fn cast_ast(source: &str, cast: &str, operand: &str) -> (usize, Value) {
    let mut ast = node(source, cast, "expression", "CXXStaticCast");
    let mut ty = node(source, "<int>", "type", "Builtin");
    ty["range"]["start"]["character"] =
        json!(ty["range"]["start"]["character"].as_u64().unwrap() + 1);
    ty["range"]["end"]["character"] = json!(ty["range"]["end"]["character"].as_u64().unwrap() - 1);
    ast["children"] = json!([
        ty,
        node(
            source,
            operand,
            "expression",
            if operand == "80" {
                "IntegerLiteral"
            } else {
                "BinaryOperator"
            }
        )
    ]);
    (source.find("<int>").unwrap(), ast)
}

#[tokio::test]
async fn ep1309_cast_comparison_precedence() {
    let source = "#include <stdio.h>\nint target(void) {\nreturn static_cast<int>(80) + 2 > 1 + 2;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("ORIGINAL EP1309 cast\n{source}");
    assert_eq!(compile_run(source, "cpp").as_deref(), Some("1\n"));
    let cast = cast_ast(source, "static_cast<int>(80)", "80");
    let asts = [cast.clone(), (source.find("static_cast").unwrap(), cast.1)];
    let mut failures = Vec::new();
    for all in [false, true] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "cpp");
            let result = change_with_ast(
                &ws,
                source,
                "cpp",
                "select:static_cast<int>(80) + 2 > 1",
                all,
                apply,
                force,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await;
            match result {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    println!(
                        "ACCEPTED EP1309 prefix all={all} apply={apply} force={force}\n{generated}"
                    );
                    let observed = compile_run(generated, "cpp");
                    failures.push(format!("non-subtree accepted: all={all} apply={apply} force={force} runtime={observed:?}"));
                }
                Err(error) => {
                    println!(
                        "REFUSED EP1309 prefix all={all} apply={apply} force={force}: {error}"
                    );
                    assert!(error.to_string().contains("complete executable expression"));
                    assert_eq!(ws.read("main.cpp"), source);
                }
            }
        }
    }
    // Every genuine subtree stays available; the comparison's right arithmetic
    // subtree is deliberately different from the invalid cast-headed prefix.
    for (selected, custom_value, expected) in [
        ("static_cast<int>(80)", "0", "0\n"),
        ("static_cast<int>(80) + 2", "0", "0\n"),
        ("1 + 2", "100", "0\n"),
        ("static_cast<int>(80) + 2 > 1 + 2", "7", "7\n"),
        ("80", "0", "0\n"),
    ] {
        for ext in ["cpp", "h"] {
            let ws = workspace(source, ext);
            let plan = change_with_ast(
                &ws,
                source,
                ext,
                &format!("select:{selected}"),
                false,
                true,
                false,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await
            .unwrap();
            assert_eq!(plan.replaced, 1);
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace(
                &format!("target({selected})"),
                &format!("target({custom_value})"),
            );
            assert_ne!(&custom, generated);
            println!(
                "PUBLIC EP1309 subtree ext={ext} selected={selected}\n{generated}\nCUSTOM\n{custom}"
            );
            assert_eq!(run(generated, ext), "1\n");
            assert_eq!(run(&custom, ext), expected);
            assert_eq!(ws.read(&format!("main.{ext}")), *generated);
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn ep1309_c_family_comment_nesting() {
    let mut failures = Vec::new();
    for ext in ["c", "cpp"] {
        for comment in ["/* /* */", "/* 80 /* 80 */"] {
            let source = format!(
                "int target(void) {{\nint a=80; {comment} return a+80;\n}}\nint main(void) {{ return target() != 160; }}\n"
            );
            println!("ORIGINAL EP1309 comment ext={ext}\n{source}");
            assert_eq!(compile_run(&source, ext).as_deref(), Some(""));
            for all in [false, true] {
                for (apply, force) in [(false, false), (true, false), (true, true)] {
                    let ws = workspace(&source, ext);
                    match change_typed(&ws, &source, ext, "80;", all, apply, force, false, "int")
                        .await
                    {
                        Ok(plan) => {
                            let generated = &plan.rewritten[0].1;
                            let expected = if all { 20 } else { 90 };
                            let custom = generated
                                .replace("target(80)", "target(10)")
                                .replace("!= 160", &format!("!= {expected}"));
                            println!(
                                "PUBLIC EP1309 comment ext={ext} all={all} apply={apply} force={force}\n{generated}\nCUSTOM\n{custom}"
                            );
                            assert_eq!(plan.replaced, if all { 2 } else { 1 });
                            assert!(generated.contains(comment));
                            assert_eq!(compile_run(generated, ext).as_deref(), Some(""));
                            assert_eq!(compile_run(&custom, ext).as_deref(), Some(""));
                            assert_eq!(
                                ws.read(&format!("main.{ext}")).as_str(),
                                if apply {
                                    generated.as_str()
                                } else {
                                    source.as_str()
                                }
                            );
                        }
                        Err(error) => {
                            println!(
                                "REFUSED EP1309 comment ext={ext} all={all} apply={apply} force={force}: {error}"
                            );
                            failures.push(format!("valid {ext} comment refused: {error}"));
                            assert_eq!(ws.read(&format!("main.{ext}")), source);
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn ep1309_c_identifier_is_not_a_cpp_cast() {
    let source = "#include <stdio.h>\nint static_cast=80; int x=2;\nint target(void) {\nreturn static_cast < x > (1) + 2;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("ORIGINAL EP1309 C identifier\n{source}");
    assert_eq!(compile_run(source, "c").as_deref(), Some("0\n"));
    let use_at = source.find("static_cast <").unwrap();
    let defs = [
        (use_at, source.find("static_cast=").unwrap()),
        (source.find("x >").unwrap(), source.find("x=2").unwrap()),
    ];
    for force in [false, true] {
        let ws = workspace(source, "c");
        let result = change_resolved(
            &ws,
            source,
            "c",
            "select:static_cast < x > (1)",
            false,
            true,
            force,
            false,
            "int",
            &defs,
        )
        .await;
        assert!(result.is_err(), "non-subtree C comparison accepted");
        assert_eq!(ws.read("main.c"), source);
    }
    let ws = workspace(source, "c");
    let selected = "static_cast < x";
    let plan = change_resolved(
        &ws,
        source,
        "c",
        &format!("select:{selected}"),
        false,
        true,
        false,
        false,
        "int",
        &defs,
    )
    .await
    .unwrap();
    let generated = &plan.rewritten[0].1;
    let custom = generated.replace(&format!("target({selected})"), "target(10)");
    println!("PUBLIC EP1309 ordinary C comparison\n{generated}\nCUSTOM\n{custom}");
    assert_eq!(compile_run(generated, "c").as_deref(), Some("0\n"));
    assert_eq!(compile_run(&custom, "c").as_deref(), Some("1\n"));
}

#[tokio::test]
async fn ep1309_swift_nested_comments_remain_opaque() {
    let source = "func target() -> Int {\nlet a=80; /* outer /* 80 */ inner 80 */ return a+80\n}\nprint(target())\n";
    let ws = workspace(source, "swift");
    let plan = change_typed(
        &ws, source, "swift", "80;", true, false, false, false, "Int",
    )
    .await
    .unwrap();
    assert_eq!(plan.replaced, 2);
    assert!(
        plan.rewritten[0]
            .1
            .contains("/* outer /* 80 */ inner 80 */")
    );
    assert_eq!(ws.read("main.swift"), source);
}

#[tokio::test]
async fn ep1309_bounded_cast_operand_subtrees() {
    let source = "#include <stdio.h>\nint target(void) {\nreturn static_cast<int>(80 + 2 > 1 + 2) + 2;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("ORIGINAL EP1309 bounded operand\n{source}");
    assert_eq!(compile_run(source, "cpp").as_deref(), Some("3\n"));
    let asts = [cast_ast(
        source,
        "static_cast<int>(80 + 2 > 1 + 2)",
        "80 + 2 > 1 + 2",
    )];
    for (selected, custom_value, expected) in [
        ("80 + 2", "0", "2\n"),
        ("1 + 2", "100", "2\n"),
        ("80 + 2 > 1 + 2", "0", "2\n"),
        ("static_cast<int>(80 + 2 > 1 + 2)", "7", "9\n"),
        ("static_cast<int>(80 + 2 > 1 + 2) + 2", "7", "7\n"),
    ] {
        let ws = workspace(source, "cpp");
        let plan = change_with_ast(
            &ws,
            source,
            "cpp",
            &format!("select:{selected}"),
            true,
            true,
            false,
            false,
            "int",
            &[],
            &[],
            &asts,
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace(
            &format!("target({selected})"),
            &format!("target({custom_value})"),
        );
        println!(
            "PUBLIC EP1309 bounded operand selected={selected}\n{generated}\nCUSTOM\n{custom}"
        );
        assert_eq!(plan.replaced, 1);
        assert_eq!(compile_run(generated, "cpp").as_deref(), Some("3\n"));
        assert_eq!(compile_run(&custom, "cpp").as_deref(), Some(expected));
    }
    for selected in ["80 + 2 > 1", "static_cast<int>(80 + 2 > 1 + 2) +"] {
        for force in [false, true] {
            let ws = workspace(source, "cpp");
            let error = change_with_ast(
                &ws,
                source,
                "cpp",
                &format!("select:{selected}"),
                true,
                true,
                force,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await
            .unwrap_err();
            println!("REFUSED EP1309 operand prefix selected={selected} force={force}: {error}");
            assert!(error.to_string().contains("complete executable expression"));
            assert_eq!(ws.read("main.cpp"), source);
        }
    }
}
