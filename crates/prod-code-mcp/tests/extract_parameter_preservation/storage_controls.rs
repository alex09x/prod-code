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
async fn c_static_initializers_remain_constant() {
    let mut failures = Vec::new();
    for declaration in [
        "static int k=1 ? 80 : 0;",
        "static const int k=(1 ? (0 ? 0 : 80) : 0);",
        "static int k=80, j=80; (void)j;",
        "static int k[]={1 ? 80 : 0};",
    ] {
        let returned = if declaration.contains("[]") {
            "k[0]"
        } else {
            "k"
        };
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {declaration} return a+{returned};\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE C static\n{source}");
        assert_eq!(run(&source, "c"), "160\n");
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, "c");
            match change_typed(&ws, &source, "c", "80;", true, apply, force, false, "int").await {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    let custom = generated.replace("target(80)", "target(10)");
                    assert_ne!(&custom, generated);
                    println!(
                        "GENERATED C static apply={apply} force={force} replaced={}\n{generated}\nCUSTOM C static\n{custom}",
                        plan.replaced
                    );
                    let default = std::panic::catch_unwind(|| run(generated, "c"));
                    let customized = std::panic::catch_unwind(|| run(&custom, "c"));
                    if plan.replaced != 1
                        || !generated.contains(declaration)
                        || !default.is_ok_and(|s| s == "160\n")
                        || !customized.is_ok_and(|s| s == "90\n")
                    {
                        failures.push(format!(
                            "{declaration} apply={apply} force={force}: invalid replacement"
                        ));
                    }
                }
                Err(e) => failures.push(format!("{declaration}: {e}")),
            }
        }
        let selected = if declaration.contains("80 :") {
            "80 :"
        } else if declaration.contains("80)") {
            "80)"
        } else {
            "80,"
        };
        for force in [false, true] {
            let ws = workspace(&source, "c");
            match change_typed(
                &ws, &source, "c", selected, false, true, force, false, "int",
            )
            .await
            {
                Err(e) => {
                    println!("REFUSED C static force={force}: {e}");
                    assert_eq!(ws.read("main.c"), source);
                }
                Ok(_) => failures.push(format!(
                    "selected static accepted: {declaration} force={force}"
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn header_constexpr_requires_analyzer_declaration() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; constexpr int k=1 ? 80 : 0; return a+k;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE header constexpr\n{source}");
    assert_eq!(run(source, "h"), "160\n");
    let at = source.find("constexpr").unwrap();
    let ast = json!({"role":"declaration", "kind":"Var", "detail":"k",
        "arcana":"VarDecl <source> k 'const int' constexpr cinitInt 80\n"});
    let mut failures = Vec::new();
    for (needle, all) in [("80;", true), ("80 :", false)] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "h");
            let result = change_with_ast(
                &ws,
                source,
                "h",
                needle,
                all,
                apply,
                force,
                false,
                "int",
                &[],
                &[],
                &[(at, ast.clone())],
            )
            .await;
            match result {
                Ok(plan) if all => {
                    let generated = &plan.rewritten[0].1;
                    let custom = generated.replace("target(80)", "target(10)");
                    assert_ne!(&custom, generated);
                    println!(
                        "GENERATED header constexpr apply={apply} force={force} replaced={}\n{generated}\nCUSTOM header constexpr\n{custom}",
                        plan.replaced
                    );
                    let default = std::panic::catch_unwind(|| run(generated, "h"));
                    let customized = std::panic::catch_unwind(|| run(&custom, "h"));
                    if plan.replaced != 1
                        || !default.is_ok_and(|s| s == "160\n")
                        || !customized.is_ok_and(|s| s == "90\n")
                    {
                        failures.push(format!(
                            "header constexpr apply={apply} force={force}: invalid replacement"
                        ));
                    }
                }
                Err(e) if !all => {
                    println!("REFUSED header constexpr: {e}");
                    assert_eq!(ws.read("main.h"), source);
                }
                result => failures.push(format!(
                    "header constexpr all={all} apply={apply} force={force}: {result:?}"
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn c_callable_assertion_names_keep_runtime_arguments() {
    let mut failures = Vec::new();
    for name in ["static_assert", "alignas"] {
        let source = format!(
            "#include <stdio.h>\nint seen; void sink(int n) {{ seen=n; }}\nint target(void) {{\nvoid (*{name})(int)=sink; int a=80; {name}(80); return a+seen;\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE C callable {name}\n{source}");
        assert_eq!(run(&source, "c"), "160\n");
        let at = source.find(&format!("{name}(80)")).unwrap();
        let hover = format!("### variable `{name}`\n\n```c\nvoid (*{name})(int)\n```");
        // Shape measured from live clangd. Call structure proves runtime use;
        // neither a variable heading nor an absent macro proves it alone.
        let ast = json!({"role":"expression", "kind":"Call", "children":[
            {"role":"expression", "kind":"ImplicitCast", "children":[
                {"role":"expression", "kind":"DeclRef", "detail":name} ]},
            {"role":"expression", "kind":"IntegerLiteral", "detail":"80"} ]});
        for (needle, all, count, expected) in [("80;", true, 2, "20\n"), ("80)", false, 1, "90\n")]
        {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, "c");
                match change_with_ast(
                    &ws,
                    &source,
                    "c",
                    needle,
                    all,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[(at, &hover)],
                    &[(at, ast.clone())],
                )
                .await
                {
                    Ok(plan) => {
                        let generated = &plan.rewritten[0].1;
                        let custom = generated.replace("target(80)", "target(10)");
                        assert_ne!(&custom, generated);
                        println!(
                            "GENERATED C callable {name} all={all} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM C callable\n{custom}",
                            plan.replaced
                        );
                        let default = run(generated, "c");
                        let customized = run(&custom, "c");
                        if plan.replaced != count || default != "160\n" || customized != expected {
                            failures.push(format!("{name} all={all}: incorrect runtime"));
                        }
                    }
                    Err(e) => {
                        failures.push(format!("{name} all={all} apply={apply} force={force}: {e}"))
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
