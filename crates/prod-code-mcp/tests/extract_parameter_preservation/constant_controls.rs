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
async fn c11_static_assert_function_is_runtime() {
    let source = "#include <stdio.h>\nint seen; void static_assert(int n) { seen=n; }\nint target(void) {\nint a=80; static_assert(80); return a+seen;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE C11 runtime assertion name\n{source}");
    assert_eq!(run(source, "c"), "160\n");
    let at = source.find("static_assert(80)").unwrap();
    let hovers = [(
        at,
        "### function static_assert\n\n```c\nvoid static_assert(int n)\n```",
    )];
    let mut failures = Vec::new();
    for (needle, all, count, expected) in [("80;", true, 2, "20\n"), ("80)", false, 1, "90\n")] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "c");
            match change_analyzed(
                &ws,
                source,
                "c",
                needle,
                all,
                apply,
                force,
                false,
                "int",
                &[],
                &hovers,
            )
            .await
            {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    let custom = generated.replace("target(80)", "target(10)");
                    println!(
                        "GENERATED runtime assertion all={all} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM runtime assertion\n{custom}",
                        plan.replaced
                    );
                    let default = run(generated, "c");
                    let actual = run(&custom, "c");
                    if plan.replaced != count || default != "160\n" || actual != expected {
                        failures.push(format!("all={all} apply={apply} force={force}: replaced={} default={default:?} custom={actual:?}", plan.replaced));
                    }
                }
                Err(e) => failures.push(format!("all={all} apply={apply} force={force}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn labels_after_statement_prefixes_are_separate_names() {
    let mut failures = Vec::new();
    for ext in ["c", "cpp"] {
        for body in [
            "if (1) limit: return 80; return 0;",
            "if (0) return 0; else limit: return 80;",
            "do limit: return 80; while (0);",
            "for (;;) limit: return 80;",
            "while (1) limit: return 80; return 0;",
            "if (1 ? 1 : 0) limit: return 80; return 0;",
        ] {
            let source = format!(
                "#include <stdio.h>\nint target(void) {{\n{body}\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
            );
            println!("SOURCE statement label {ext}\n{source}");
            assert_eq!(run(&source, ext), "80\n");
            for all in [false, true] {
                let ws = workspace(&source, ext);
                match change_typed(&ws, &source, ext, "80;", all, true, false, false, "int").await {
                    Ok(plan) => {
                        let generated = &plan.rewritten[0].1;
                        let custom = generated.replace("target(80)", "target(10)");
                        println!(
                            "GENERATED label {ext} all={all}\n{generated}\nCUSTOM label\n{custom}"
                        );
                        assert_eq!(run(generated, ext), "80\n");
                        assert_eq!(run(&custom, ext), "10\n");
                        assert_eq!(plan.replaced, 1);
                    }
                    Err(e) => failures.push(format!("{ext} {body} all={all}: {e}")),
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn constexpr_initializers_remain_constant() {
    let mut failures = Vec::new();
    for declaration in [
        "constexpr int k = true ? 80 : 0;",
        "constexpr int k = (true ? (false ? 0 : 80) : 0);",
        "constexpr int k{true ? 80 : 0};",
        "constexpr int k = 80, j = 80; (void)j;",
    ] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {declaration} return a+k;\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE constexpr\n{source}");
        assert_eq!(run(&source, "cpp"), "160\n");
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, "cpp");
            let plan = change_typed(&ws, &source, "cpp", "80;", true, apply, force, false, "int")
                .await
                .unwrap();
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target(80)", "target(10)");
            println!(
                "GENERATED constexpr apply={apply} force={force} replaced={}\n{generated}\nCUSTOM constexpr\n{custom}",
                plan.replaced
            );
            let default = std::panic::catch_unwind(|| run(generated, "cpp"));
            let customized = std::panic::catch_unwind(|| run(&custom, "cpp"));
            if plan.replaced != 1
                || !generated.contains(declaration)
                || !default.as_ref().is_ok_and(|s| s == "160\n")
                || !customized.as_ref().is_ok_and(|s| s == "90\n")
            {
                failures.push(format!(
                    "{declaration} apply={apply} force={force}: replaced={}",
                    plan.replaced
                ));
            }
        }
        for force in [false, true] {
            let needle = if declaration.contains("80)") {
                "80)"
            } else if declaration.contains("80 :") {
                "80 :"
            } else {
                "80,"
            };
            let ws = workspace(&source, "cpp");
            let result = change_typed(
                &ws, &source, "cpp", needle, false, true, force, false, "int",
            )
            .await;
            match result {
                Err(e) => {
                    println!("REFUSED constexpr force={force}: {e}");
                    assert_eq!(ws.read("main.cpp"), source);
                }
                Ok(_) => failures.push(format!(
                    "{declaration}: selected constant accepted force={force}"
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn unknown_c_assertion_context_refuses_before_writes() {
    let source = "#include <stdio.h>\nint seen; void static_assert(int n) { seen=n; }\nint target(void) {\nint a=80; static_assert(80); return a+seen;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        for needle in ["80;", "80)"] {
            let ws = workspace(source, "c");
            let error = change_typed(&ws, source, "c", needle, true, apply, force, false, "int")
                .await
                .unwrap_err();
            println!("REFUSED unknown C assertion apply={apply} force={force}: {error}");
            assert!(
                error
                    .to_string()
                    .contains("incomplete C assertion coverage")
            );
            assert_eq!(ws.read("main.c"), source);
        }
    }
}

#[tokio::test]
async fn constexpr_condition_keeps_runtime_branches() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; if constexpr (80 == 80) { return a + (true ? 80 : 0); } return 0;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE constexpr condition\n{source}");
    assert_eq!(run(source, "cpp"), "160\n");
    let ws = workspace(source, "cpp");
    let plan = change_typed(&ws, source, "cpp", "80;", true, true, false, false, "int")
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    let custom = generated.replace("target(80)", "target(10)");
    println!(
        "GENERATED constexpr condition replaced={}\n{generated}\nCUSTOM constexpr condition\n{custom}",
        plan.replaced
    );
    assert_eq!(run(generated, "cpp"), "160\n");
    assert_eq!(run(&custom, "cpp"), "20\n");
    assert_eq!(plan.replaced, 2);
    assert!(generated.contains("if constexpr (80 == 80)"));
}
