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

/// Run both argument choices even when a pre-repair plan has the wrong count.
fn check_program(source: &str, generated: &str, ext: &str, argument: &str, custom: &str) {
    println!("SOURCE {ext}\n{source}\nGENERATED {ext}\n{generated}");
    let original = run(source, ext);
    let default = run(generated, ext);
    let customized = generated.replace(argument, custom);
    assert_ne!(customized, generated);
    println!("CUSTOM {ext}\n{customized}");
    let actual = run(&customized, ext);
    assert_eq!(default, original);
    assert_eq!(actual, "20\n");
}

#[tokio::test]
async fn builtin_operators_resolve_operands_only() {
    let mut failures = Vec::new();
    for ext in ["c", "cpp"] {
        for expression in ["sizeof(80)", "sizeof(int)", "sizeof(x)"] {
            let source = format!(
                "#include <stdio.h>\nint target(int x) {{\nreturn {expression} + {expression};\n}}\nint main(void) {{ int x = 4; printf(\"%d\\n\", target(x)); }}\n"
            );
            println!("SOURCE builtin {ext} {expression}\n{source}");
            assert_eq!(run(&source, ext), "8\n");
            let definitions = if expression == "sizeof(x)" {
                source
                    .match_indices("sizeof(x)")
                    .map(|(at, _)| (at + 7, source.find("x)").unwrap()))
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let ws = workspace(&source, ext);
            match change_resolved(
                &ws,
                &source,
                ext,
                &format!("select:{expression}"),
                true,
                true,
                false,
                false,
                "int",
                &definitions,
            )
            .await
            {
                Ok(plan) => {
                    let generated = ws.read(&format!("main.{ext}"));
                    check_program(
                        &source,
                        &generated,
                        ext,
                        &format!("target(x, {expression})"),
                        "target(x, 10)",
                    );
                    assert_eq!(plan.replaced, 2);
                }
                Err(e) => failures.push(format!("{ext} {expression}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn conditional_case_labels_stay_constant() {
    let mut failures = Vec::new();
    for ext in ["c", "cpp"] {
        for label in ["1 ? 80 : 0", "1 ? (0 ? 0 : 80) : 0", "1 ? 0 ? 0 : 80 : 0"] {
            let source = format!(
                "#include <stdio.h>\nint target(void) {{\nint a = 80; switch (80 ) {{ case {label}: return a; default: return 0; }}\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
            );
            println!("SOURCE case {ext}\n{source}");
            assert_eq!(run(&source, ext), "80\n");
            let ws = workspace(&source, ext);
            let plan = change_typed(&ws, &source, ext, "80;", true, true, false, false, "int")
                .await
                .unwrap();
            let generated = ws.read(&format!("main.{ext}"));
            println!("GENERATED case replaced={}\n{generated}", plan.replaced);
            let custom = generated.replace("target(80)", "target(10)");
            println!("CUSTOM case\n{custom}");
            // Keep complete before programs even when the compiler will reject the plan.
            if plan.replaced != 2 || !generated.contains(&format!("case {label}:")) {
                let _ = std::panic::catch_unwind(|| run(&generated, ext));
                let _ = std::panic::catch_unwind(|| run(&custom, ext));
                failures.push(format!("{ext} {label}: replaced={}", plan.replaced));
                continue;
            }
            assert_eq!(run(&generated, ext), "80\n");
            assert_eq!(run(&custom, ext), "0\n");
            let ws = workspace(&source, ext);
            let needle = if label.contains("80)") { "80)" } else { "80 :" };
            let error = change_typed(&ws, &source, ext, needle, false, true, true, false, "int")
                .await
                .unwrap_err();
            assert!(error.to_string().contains("complete executable expression"));
            assert_eq!(ws.read(&format!("main.{ext}")), source);
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn member_and_label_names_do_not_collide() {
    let mut failures = Vec::new();
    for ext in ["c", "cpp"] {
        for body in [
            "struct S s = {0}; return 80 + s.limit;",
            "struct S s = {0}; struct S *p = &s; return 80 + p->limit;",
            "goto limit; limit: return 80;",
            "struct limit { int n; }; struct limit s = {0}; return 80 + s.n;",
        ] {
            if ext == "cpp" && body.starts_with("struct limit") {
                // C++ tag declarations share the parameter's declaration scope.
                continue;
            }
            let source = format!(
                "#include <stdio.h>\nstruct S {{ int limit; }};\nint target(void) {{\n{body}\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
            );
            println!("SOURCE member/label {ext}\n{source}");
            assert_eq!(run(&source, ext), "80\n");
            let ws = workspace(&source, ext);
            match change_typed(
                &ws,
                &source,
                ext,
                "select:80",
                false,
                true,
                false,
                false,
                "int",
            )
            .await
            {
                Ok(plan) => {
                    let generated = ws.read(&format!("main.{ext}"));
                    println!("GENERATED member/label\n{generated}");
                    assert_eq!(run(&generated, ext), "80\n");
                    let custom = generated.replace("target(80)", "target(10)");
                    assert_ne!(custom, generated);
                    println!("CUSTOM member/label\n{custom}");
                    assert_eq!(run(&custom, ext), "10\n");
                    assert_eq!(plan.replaced, 1);
                }
                Err(e) => failures.push(format!("{ext} {body}: {e}")),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn unknown_operands_and_possible_capture_refuse_before_writes() {
    for ext in ["c", "cpp"] {
        for (params, body, selected) in [
            ("int x", "return sizeof(x);", "sizeof(x)"),
            ("void", "int limit = 1; return 80 + limit;", "80"),
            (
                "void",
                "int a = 80; { int limit = 1; a += 80 + limit; } return a;",
                "80",
            ),
            ("int limit", "return 80;", "80"),
            ("void", "struct limit { int n; }; return 80;", "80"),
            (
                "int restrict",
                "return sizeof(restrict);",
                "sizeof(restrict)",
            ),
        ] {
            if ext == "c" && (body.starts_with("struct limit") || params == "int restrict") {
                continue;
            }
            let source = format!(
                "int target({params}) {{\n{body}\n}}\nint main(void) {{ return target(1); }}\n"
            );
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, ext);
                let result = change_typed(
                    &ws,
                    &source,
                    ext,
                    &format!("select:{selected}"),
                    true,
                    apply,
                    force,
                    false,
                    "int",
                )
                .await;
                assert!(result.is_err(), "unexpected success {ext} {source}");
                println!(
                    "REFUSED {ext} apply={apply} force={force}: {}",
                    result.unwrap_err()
                );
                assert_eq!(ws.read(&format!("main.{ext}")), source);
            }
        }
    }
}
