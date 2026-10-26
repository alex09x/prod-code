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

#[tokio::test]
async fn cpp_normalized_shadowing_uses_declaration_identity() {
    let source = "#include <iostream>\nint target(int x) {\nint a = x + 1; int b = 0; { int x = 10; b = x+1; } return a+b;\n}\nint main() { int x = 4; std::cout << target(x) << '\\n'; }\n";
    println!("SOURCE C++ shadow\n{source}");
    assert_eq!(run(source, "cpp"), "16\n");
    let ws = workspace(source, "cpp");
    let definitions = [
        (source.find("x + 1").unwrap(), source.find("x)").unwrap()),
        (source.find("x+1").unwrap(), source.find("x = 10").unwrap()),
    ];
    let plan = change_resolved(
        &ws,
        source,
        "cpp",
        "select:x + 1",
        true,
        true,
        false,
        false,
        "int",
        &definitions,
    )
    .await
    .unwrap();
    let generated = ws.read("main.cpp");
    println!(
        "GENERATED C++ shadow replaced={}\n{generated}",
        plan.replaced
    );
    let default = run(&generated, "cpp");
    let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
    assert_ne!(custom, generated);
    println!("CUSTOM C++ shadow\n{custom}");
    let customized = run(&custom, "cpp");
    assert_eq!(
        (plan.replaced, default.as_str(), customized.as_str()),
        (1, "16\n", "31\n")
    );
}

#[tokio::test]
async fn ternary_branch_values_are_executable() {
    let source = "function target() {\nconst a = 80;\nconst b = true ? 80 : 0;\nreturn a + b;\n}\nconsole.log(target());\n";
    println!("SOURCE ternary\n{source}");
    assert_eq!(run(source, "js"), "160\n");
    let mut failures = Vec::new();
    for (needle, all, count, expected) in [("80;", true, 2, "20\n"), ("80 :", false, 1, "90\n")] {
        let ws = workspace(source, "js");
        match change(&ws, source, "js", needle, all, true, false, false).await {
            Ok(plan) => {
                let generated = ws.read("main.js");
                println!(
                    "GENERATED ternary all={all} replaced={}\n{generated}",
                    plan.replaced
                );
                assert_eq!(run(&generated, "js"), "160\n");
                let custom = generated.replace("target(80)", "target(10)");
                assert_ne!(custom, generated);
                println!("CUSTOM ternary\n{custom}");
                let actual = run(&custom, "js");
                if plan.replaced != count || actual != expected {
                    failures.push(format!(
                        "all={all} replaced={} actual={actual:?}",
                        plan.replaced
                    ));
                }
            }
            Err(e) => failures.push(format!("all={all} REFUSED {e}")),
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn satisfies_else_do_calls_keep_runtime_arguments() {
    let mut failures = Vec::new();
    for statement in [
        "if (false) {} else satisfies(80);",
        "do satisfies(80); while (false);",
    ] {
        for template in [false, true] {
            let statement = if template {
                statement.replace("(80)", "(`${80}`)")
            } else {
                statement.into()
            };
            let source = format!(
                "let seen = 0;\nfunction satisfies(v: number | string) {{ seen = Number(v); }}\nfunction target() {{\nconst a = 80;\n{statement}\nreturn a + seen;\n}}\nconsole.log(target());\n"
            );
            println!("SOURCE TypeScript runtime\n{source}");
            assert_eq!(run(&source, "ts"), "160\n");
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, "ts");
                match change_typed(
                    &ws, &source, "ts", "80;", true, apply, force, false, "number",
                )
                .await
                {
                    Ok(plan) => {
                        let generated = &plan.rewritten[0].1;
                        println!(
                            "GENERATED TypeScript apply={apply} force={force} replaced={}\n{generated}",
                            plan.replaced
                        );
                        assert_eq!(run(generated, "ts"), "160\n");
                        let custom = generated.replace("target(80)", "target(10)");
                        assert_ne!(&custom, generated);
                        println!("CUSTOM TypeScript\n{custom}");
                        let actual = run(&custom, "ts");
                        if template || plan.replaced != 2 || actual != "20\n" {
                            failures.push(format!("{statement} apply={apply} force={force} replaced={} actual={actual:?}", plan.replaced));
                        }
                    }
                    Err(e) if template => {
                        println!("REFUSED TypeScript runtime template: {e}");
                        assert!(e.to_string().contains("incomplete expression coverage"));
                        assert_eq!(ws.read("main.ts"), source);
                    }
                    Err(e) => failures.push(format!("{statement}: {e}")),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn private_keyword_division_keeps_unrelated_single_selection() {
    let mut failures = Vec::new();
    for ext in ["js", "ts"] {
        for name in ["if", "while", "for", "with", "switch", "catch"] {
            let source = format!(
                "class C {{\n#{name}() {{ return 80; }}\ntarget() {{\nconst a = 80;\nconst b = `${{this.#{name}() / 2}}`;\nreturn a + ':' + b;\n}}\n}}\nconsole.log(new C().target());\n"
            );
            println!("SOURCE private division\n{source}");
            assert_eq!(run(&source, ext), "80:40\n");
            let ws = workspace(&source, ext);
            match change_typed(
                &ws,
                &source,
                ext,
                "80;\nconst b",
                false,
                true,
                false,
                false,
                "number",
            )
            .await
            {
                Ok(plan) => {
                    let generated = ws.read(&format!("main.{ext}"));
                    println!(
                        "GENERATED private division {ext} replaced={}\n{generated}",
                        plan.replaced
                    );
                    assert_eq!(plan.replaced, 1);
                    assert_eq!(run(&generated, ext), "80:40\n");
                    let custom = generated.replace(".target(80)", ".target(10)");
                    assert_ne!(custom, generated);
                    println!("CUSTOM private division\n{custom}");
                    assert_eq!(run(&custom, ext), "10:40\n");
                }
                Err(e) => failures.push(format!("{ext} {name} REFUSED {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
