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
use serde_json::json;

#[tokio::test]
async fn runtime_initializers_remain_replaceable() {
    for (ext, storage) in [("c", ""), ("cpp", ""), ("h", ""), ("cpp", "static ")] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {storage}int k=1 ? 80 : 0; return a+k;\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE runtime initializer {ext} {storage}\n{source}");
        assert_eq!(run(&source, ext), "160\n");
        let ws = workspace(&source, ext);
        let plan = change_typed(&ws, &source, ext, "80;", true, true, false, false, "int")
            .await
            .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(80)", "target(10)");
        assert_ne!(&custom, generated);
        println!(
            "GENERATED runtime initializer replaced={}\n{generated}\nCUSTOM runtime initializer\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, 2);
        assert_eq!(run(generated, ext), "160\n");
        assert_eq!(run(&custom, ext), "20\n");
    }
}

#[tokio::test]
async fn c_constexpr_names_use_resolved_identity() {
    for (prefix, declaration, returned) in [
        ("", "int constexpr=1 ? 80 : 0;", "constexpr"),
        ("typedef int constexpr;\n", "constexpr k=1 ? 80 : 0;", "k"),
        (
            "int constexpr(int n) { return n; }\n",
            "int k=constexpr(80);",
            "k",
        ),
    ] {
        let source = format!(
            "#include <stdio.h>\n{prefix}int target(void) {{\nint a=80; {declaration} return a+{returned};\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE C constexpr name\n{source}");
        assert_eq!(run(&source, "c"), "160\n");
        let definition = source.find("constexpr").unwrap();
        let definitions = source
            .match_indices("constexpr")
            .map(|(at, _)| (at, definition))
            .collect::<Vec<_>>();
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, "c");
            let plan = change_resolved(
                &ws,
                &source,
                "c",
                "80;",
                true,
                apply,
                force,
                false,
                "int",
                &definitions,
            )
            .await
            .unwrap();
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target(80)", "target(10)");
            assert_ne!(&custom, generated);
            println!(
                "GENERATED C constexpr name replaced={}\n{generated}\nCUSTOM C constexpr name\n{custom}",
                plan.replaced
            );
            assert_eq!(plan.replaced, 2);
            assert_eq!(run(generated, "c"), "160\n");
            assert_eq!(run(&custom, "c"), "20\n");
        }
    }
}

#[tokio::test]
async fn header_constexpr_groups_and_conditions_keep_runtime_values() {
    let var = json!({"role":"declaration", "kind":"Var", "detail":"k",
        "arcana":"VarDecl <source> k 'const int' constexpr cinitInt 80\n"});
    for (statement, ast, count, expected) in [
        (
            "constexpr int k=80, j=80; (void)j; return a+k;",
            json!({"role":"statement", "kind":"Decl", "children":[var.clone(),var.clone()]}),
            1,
            "90\n",
        ),
        (
            "static constexpr int k=1 ? 80 : 0; return a+k;",
            var,
            1,
            "90\n",
        ),
        (
            "if constexpr (80 == 80) { return a+(1 ? 80 : 0); } return 0;",
            json!({"role":"statement", "kind":"If", "arcana":"IfStmt <source> constexpr"}),
            2,
            "20\n",
        ),
    ] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {statement}\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE header constant group\n{source}");
        assert_eq!(run(&source, "h"), "160\n");
        let at = source
            .find(if statement.starts_with("static") {
                "static"
            } else {
                "constexpr"
            })
            .unwrap();
        let ws = workspace(&source, "h");
        let plan = change_with_ast(
            &ws,
            &source,
            "h",
            "80;",
            true,
            true,
            false,
            false,
            "int",
            &[],
            &[],
            &[(at, ast)],
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(80)", "target(10)");
        assert_ne!(&custom, generated);
        println!(
            "GENERATED header constant group replaced={}\n{generated}\nCUSTOM header constant group\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, count);
        assert_eq!(run(generated, "h"), "160\n");
        assert_eq!(run(&custom, "h"), expected);
    }
}

#[tokio::test]
async fn unknown_header_storage_or_constexpr_refuses_before_writes() {
    for word in ["static", "constexpr"] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {word} int k=1 ? 80 : 0; return a+k;\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        assert_eq!(run(&source, "h"), "160\n");
        let at = source.find(word).unwrap();
        // Real C and C++ static Var nodes have the same shape, and clangd
        // prints cpp fences for both. This is intentionally insufficient proof.
        let ast = json!({"kind":"Var", "role":"declaration", "detail":"k",
            "arcana":"VarDecl <source> k 'int' static cinit"});
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            for (needle, all) in [("80;", true), ("80 :", false)] {
                let ws = workspace(&source, "h");
                let error = change_with_ast(
                    &ws,
                    &source,
                    "h",
                    needle,
                    all,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[(at, "```cpp\nstatic int k = 80;\n```")],
                    &[(at, ast.clone())],
                )
                .await
                .unwrap_err();
                println!("REFUSED unknown {word} all={all} apply={apply} force={force}: {error}");
                assert!(error.to_string().contains("incomplete C/C++"));
                assert_eq!(ws.read("main.h"), source);
            }
        }
    }
}
