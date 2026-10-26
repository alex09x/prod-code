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
use serde_json::{Value, json};

// Minimal scripted replies use the node roles/ranges confirmed by the retained
// live clangd angle-token probe. They do not simulate compiler diagnostics.
fn range(source: &str, start: usize, len: usize) -> Value {
    let (line, col) = position(source, start);
    let (end_line, end_col) = position(source, start + len);
    json!({"start":{"line":line-1,"character":col-1},
           "end":{"line":end_line-1,"character":end_col-1}})
}

fn template(source: &str, marker: &str, argument: &str, function: bool) -> (usize, Value) {
    let start = source.find(marker).unwrap();
    let arg = start + marker.find(argument).unwrap();
    (
        start + marker.find('<').unwrap(),
        json!({
            "kind":if function {"DeclRef"} else {"TemplateSpecialization"},
            "role":if function {"expression"} else {"type"},
            "range":range(source, start, marker.len()),
            "children":[{"kind":"Expression","role":"template argument",
                         "range":range(source, arg, argument.len())}]
        }),
    )
}

#[tokio::test]
async fn cpp_template_branch_remains_constant() {
    let mut failures = Vec::new();
    for ext in ["cpp", "h"] {
        for (marker, argument, function, returned) in [
            (
                "std::array<int, true ? 80 : 1>",
                "true ? 80 : 1",
                false,
                "values.size()",
            ),
            (
                "std::array<int, (true ? 80 : 1)>",
                "(true ? 80 : 1)",
                false,
                "values.size()",
            ),
            ("constant<true ? 80 : 1>", "true ? 80 : 1", true, "values"),
            (
                "constant<(true ? 80 : 1)>",
                "(true ? 80 : 1)",
                true,
                "values",
            ),
            (
                "std::array<std::array<int, 80>, 1>",
                "std::array<int, 80>",
                false,
                "values[0].size()",
            ),
        ] {
            let declaration = if function {
                format!("int values={marker}();")
            } else {
                format!("{marker} values{{}};")
            };
            let source = format!(
                "#include <array>\n#include <stdio.h>\ntemplate<int N> int constant() {{ return N; }}\nint target(void) {{\nint a=80; {declaration} return a+{returned};\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
            );
            println!("SOURCE template branch {ext}\n{source}");
            assert_eq!(run(&source, ext), "160\n");
            let asts = [template(&source, marker, argument, function)];
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, ext);
                let plan = change_with_ast(
                    &ws,
                    &source,
                    ext,
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
                assert_ne!(&custom, generated);
                println!(
                    "GENERATED template apply={apply} force={force} replaced={}\n{generated}\nCUSTOM template\n{custom}",
                    plan.replaced
                );
                let default = std::panic::catch_unwind(|| run(generated, ext));
                let customized = std::panic::catch_unwind(|| run(&custom, ext));
                if plan.replaced != 1
                    || !generated.contains(marker)
                    || !default.as_ref().is_ok_and(|s| s == "160\n")
                    || !customized.as_ref().is_ok_and(|s| s == "90\n")
                {
                    failures.push(format!(
                        "apply={apply} force={force}: replaced={}",
                        plan.replaced
                    ));
                }
            }
            for force in [false, true] {
                let ws = workspace(&source, ext);
                let needle = if marker.contains("80 :") {
                    "80 :"
                } else {
                    "80>"
                };
                let error = change_with_ast(
                    &ws,
                    &source,
                    ext,
                    needle,
                    false,
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
                println!("REFUSED selected template force={force}: {error}");
                assert!(error.to_string().contains("complete executable expression"));
                assert_eq!(ws.read(&format!("main.{ext}")), source);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn cpp_runtime_comparisons_keep_branch_values() {
    for body in [
        "bool b=4 < (true ? 80 : 0); return a+(b ? 80 : 0);",
        "bool b=4 < 80 && 80 > 0; return a+(b ? 80 : 0);",
    ] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {body}\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE comparison\n{source}");
        assert_eq!(run(&source, "cpp"), "160\n");
        let start = source.find("4 <").unwrap();
        let end = source[start..].find(';').unwrap();
        let asts = [(
            start + 2,
            json!({"role":"expression","kind":"BinaryOperator","detail":"<",
            "range":range(&source, start, end)}),
        )];
        let ws = workspace(&source, "cpp");
        let plan = change_with_ast(
            &ws,
            &source,
            "cpp",
            "80;",
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
        let custom = generated.replace("target(80)", "target(10)");
        println!(
            "GENERATED comparison replaced={}\n{generated}\nCUSTOM comparison\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, if body.contains("&&") { 4 } else { 3 });
        assert_eq!(run(generated, "cpp"), "160\n");
        assert_eq!(run(&custom, "cpp"), "20\n");
    }
}

#[tokio::test]
async fn cpp_unknown_template_context_refuses_before_writes() {
    let source = "#include <array>\nint target(void) {\nint a=80; std::array<int, true ? 80 : 1> values{}; return a+values.size();\n}\nint main(void) { return target(); }\n";
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        for needle in ["80;", "80 :"] {
            let ws = workspace(source, "cpp");
            let error = change_typed(&ws, source, "cpp", needle, true, apply, force, false, "int")
                .await
                .unwrap_err();
            println!("REFUSED unknown template apply={apply} force={force}: {error}");
            assert!(
                error
                    .to_string()
                    .contains("incomplete C/C++ template coverage")
            );
            assert_eq!(ws.read("main.cpp"), source);
        }
    }
}

#[tokio::test]
async fn cpp_empty_template_call_keeps_surrounding_literals() {
    let source = "#include <stdio.h>\ntemplate<int N=80> int constant() { return N; }\nint target(void) {\nint a=80; return a+constant<>()+80;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE empty template\n{source}");
    assert_eq!(run(source, "cpp"), "240\n");
    let start = source.find("constant<>").unwrap();
    let asts = [(
        start + 8,
        json!({"role":"expression", "kind":"DeclRef",
        "range":range(source, start, "constant<>".len())}),
    )];
    let ws = workspace(source, "cpp");
    let plan = change_with_ast(
        &ws,
        source,
        "cpp",
        "80;",
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
    let custom = generated.replace("target(80)", "target(10)");
    println!("GENERATED empty template\n{generated}\nCUSTOM empty template\n{custom}");
    assert_eq!(plan.replaced, 2);
    assert_eq!(run(generated, "cpp"), "240\n");
    assert_eq!(run(&custom, "cpp"), "100\n");
}
