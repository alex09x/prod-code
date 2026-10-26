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
use std::process::Command;

#[tokio::test]
async fn go_keyword_parameter_types_reuse_the_same_binding() {
    let mut failures = Vec::new();
    for (ty, value, expression, extra) in [
        (
            "map[string]int",
            "map[string]int{\"n\": 4}",
            "x[\"n\"] + 1",
            "",
        ),
        ("chan int", "make(chan int, 4)", "cap(x) + 1", ""),
        ("func() int", "func() int { return 4 }", "x() + 1", ""),
        ("struct{ n int }", "struct{ n int }{4}", "x.n + 1", ""),
        (
            "interface{ N() int }",
            "number(4)",
            "x.N() + 1",
            "type number int\nfunc (n number) N() int { return int(n) }\n",
        ),
    ] {
        // Both a single name and a grouped name list must keep root binding identity.
        for grouped in [false, true] {
            let parameters = if grouped {
                format!("x, z {ty}")
            } else {
                format!("x {ty}")
            };
            let arguments = if grouped { "x, x" } else { "x" };
            let discard = if grouped { "_ = z;" } else { "" };
            let normalized = expression.replace(' ', "");
            let source = format!(
                "package main\nimport \"fmt\"\n{extra}func target({parameters}) int {{\na := {expression}; x, y := x, 0; b := {normalized}; _ = y; {discard} return a+b\n}}\nfunc main() {{ x := {value}; fmt.Println(target({arguments})) }}\n"
            );
            println!("SOURCE Go parameter\n{source}");
            assert_eq!(run(&source, "go"), "10\n");
            let ws = workspace(&source, "go");
            let plan = change(
                &ws,
                &source,
                "go",
                &format!("select:{expression}"),
                true,
                false,
                false,
                false,
            )
            .await
            .unwrap();
            let generated = &plan.rewritten[0].1;
            println!(
                "GENERATED Go parameter replaced={}\n{generated}",
                plan.replaced
            );
            assert_eq!(run(generated, "go"), "10\n");
            let customized = generated.replace(
                &format!("target({arguments}, {expression})"),
                &format!("target({arguments}, 10)"),
            );
            let actual = run(&customized, "go");
            println!("CUSTOMIZED Go parameter expected=20 actual={actual:?}");
            let applied = change(
                &ws,
                &source,
                "go",
                &format!("select:{expression}"),
                true,
                true,
                false,
                false,
            )
            .await
            .unwrap();
            assert!(applied.applied);
            assert_eq!(ws.read("main.go"), *generated);
            if plan.replaced != 2 || actual != "20\n" {
                failures.push(format!(
                    "{parameters}: replaced={}, customized={actual:?}",
                    plan.replaced
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

fn cpp_compiles(source: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.cpp");
    std::fs::write(&input, source).unwrap();
    let result = Command::new("c++")
        .args(["-std=c++17", "-fsyntax-only"])
        .arg(input)
        .output()
        .unwrap();
    println!(
        "C++ syntax: {} {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    result.status.success()
}

#[test]
fn cpp_concatenation_requires_the_complete_expression() {
    let mut failures = Vec::new();
    for (left, right, ty) in [
        ("R\"(a)\"", "R\"(b)\"", "char"),
        ("R\"tag(a)tag\"", "\"b\"", "char"),
        ("\"a\"", "R\"(b)\"", "char"),
        ("u8R\"(a)\"", "\"b\"", "char"),
        ("LR\"(a)\"", "L\"b\"", "wchar_t"),
    ] {
        for gap in [" ", " /* join */ "] {
            // Scripted gateways live until their runtime drops. Bound each fixture's
            // listeners instead of retaining the whole matrix in one async runtime.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
            let whole = format!("{left}{gap}{right}");
            let source = format!(
                "#include <iostream>\nconst {ty}* target() {{\nreturn {whole};\n}}\nint main() {{ std::cout << int(target()[1]) << '\\n'; }}\n"
            );
            println!("SOURCE C++ concatenation\n{source}");
            assert_eq!(run(&source, "cpp"), "98\n");
            // Prefix and suffix selections must refuse for preview, apply and force.
            for partial in [left, right] {
                for all in [false, true] {
                    for (apply, force) in [(false, false), (true, false), (true, true)] {
                        let ws = workspace(&source, "cpp");
                        match change_typed(
                            &ws,
                            &source,
                            "cpp",
                            &format!("select:{partial}"),
                            all,
                            apply,
                            force,
                            false,
                            &format!("const {ty}*"),
                        )
                        .await
                        {
                            Ok(plan) => {
                                let generated = &plan.rewritten[0].1;
                                println!(
                                    "UNEXPECTED partial C++ apply={apply} force={force}\n{generated}"
                                );
                                let compiled = cpp_compiles(generated);
                                failures.push(format!(
                                    "partial {partial} accepted, compiled={compiled}"
                                ));
                            }
                            Err(error) => {
                                println!("C++ complete-expression refusal: {error}");
                                assert!(
                                    error
                                        .to_string()
                                        .contains("not a complete executable expression")
                                );
                                assert_eq!(ws.read("main.cpp"), source);
                            }
                        }
                    }
                }
            }
            // A complete whitespace-joined expression stays useful in both modes.
            // A comment-spanning selection retains the existing trivia refusal.
            if gap == " " {
                for all in [false, true] {
                    let ws = workspace(&source, "cpp");
                    match change_typed(
                        &ws,
                        &source,
                        "cpp",
                        &format!("select:{whole}"),
                        all,
                        true,
                        false,
                        false,
                        &format!("const {ty}*"),
                    )
                    .await
                    {
                        Ok(plan) => {
                            assert_eq!(plan.replaced, 1);
                            let generated = ws.read("main.cpp");
                            assert_eq!(run(&generated, "cpp"), "98\n");
                            let argument = if ty == "wchar_t" { "L\"cd\"" } else { "\"cd\"" };
                            let customized = generated.replace(
                                &format!("target({whole})"),
                                &format!("target({argument})"),
                            );
                            assert_eq!(run(&customized, "cpp"), "100\n");
                        }
                        Err(error) => failures.push(format!("whole {whole} refused: {error}")),
                    }
                }
            }
            });
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn go_increment_and_decrement_end_statements() {
    let mut failures = Vec::new();
    for (operator, original, customized) in [("++", "85\n", "15\n"), ("--", "83\n", "13\n")] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target(x int) int {{\nx{operator}\ny := 80\nreturn x+y\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go increment\n{source}");
        assert_eq!(run(&source, "go"), original);
        for all in [false, true] {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, "go");
                match change(&ws, &source, "go", "select:80", all, apply, force, false).await {
                    Ok(plan) => {
                        let generated = &plan.rewritten[0].1;
                        println!("GENERATED Go increment\n{generated}");
                        assert_eq!(plan.replaced, 1);
                        assert_eq!(run(generated, "go"), original);
                        assert_eq!(
                            run(&generated.replace("target(x, 80)", "target(x, 10)"), "go"),
                            customized
                        );
                        assert_eq!(
                            ws.read("main.go"),
                            if apply { generated } else { &source }.as_str()
                        );
                    }
                    Err(error) => failures.push(format!(
                        "{operator} all={all} apply={apply} force={force}: {error}"
                    )),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
