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
async fn go_range_function_elements_keep_initializer_replacements() {
    let mut failures = Vec::new();
    for elements in [
        "[]func() int{factory(x+1)}",
        "[1]func() int{factory(x+1)}",
        "map[int]func() int{0: factory(x+1)}",
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nvar seen int\nfunc factory(v int) func() int {{ seen = v; return func() int {{ return v }} }}\nfunc target(x int) int {{\na := x + 1\nfor range {elements} {{}}\nreturn a + seen\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go range\n{source}");
        assert_eq!(run(&source, "go"), "10\n");
        let ws = workspace(&source, "go");
        let plan = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.go");
        println!("GENERATED Go range replaced={}\n{generated}", plan.replaced);
        assert_eq!(run(&generated, "go"), "10\n");
        let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
        assert_ne!(custom, generated);
        println!("CUSTOM Go range\n{custom}");
        let actual = run(&custom, "go");
        if plan.replaced != 2 || actual != "40\n" {
            failures.push(format!(
                "{elements}: replaced={} custom={actual:?}",
                plan.replaced
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn satisfies_runtime_calls_after_conditions_keep_numeric_arguments() {
    let mut failures = Vec::new();
    for statement in [
        "if (true) satisfies(80);",
        "while (seen === 0) satisfies(80);",
        "for (; seen === 0;) satisfies(80);",
    ] {
        let source = format!(
            "let seen = 0;\nfunction satisfies(v) {{ seen = Number(v); }}\nfunction target() {{\nconst a = 80;\n{statement}\nreturn a + seen;\n}}\nconsole.log(target());\n"
        );
        println!("SOURCE TypeScript numeric call\n{source}");
        assert_eq!(run(&source, "js"), "160\n");
        let ws = workspace(&source, "ts");
        let plan = change_typed(
            &ws, &source, "ts", "80;", true, true, false, false, "number",
        )
        .await
        .unwrap();
        let generated = ws.read("main.ts");
        println!(
            "GENERATED TypeScript replaced={}\n{generated}",
            plan.replaced
        );
        let executable = generated.replace("limit: number", "limit");
        assert_eq!(run(&executable, "js"), "160\n");
        let custom = executable.replace("target(80)", "target(10)");
        assert_ne!(custom, executable);
        println!("CUSTOM TypeScript (generated annotation erased)\n{custom}");
        let actual = run(&custom, "js");
        if plan.replaced != 2 || actual != "20\n" {
            failures.push(format!(
                "{statement}: replaced={} custom={actual:?}",
                plan.replaced
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn satisfies_runtime_calls_after_conditions_keep_template_uncertainty() {
    let mut accepted = Vec::new();
    for statement in [
        "if (true) satisfies(`${80}`);",
        "while (seen === 0) satisfies(`${80}`);",
        "for (; seen === 0;) satisfies(`${80}`);",
    ] {
        let source = format!(
            "let seen = 0;\nfunction satisfies(v) {{ seen = Number(v); }}\nfunction target() {{\nconst a = 80;\n{statement}\nreturn a + seen;\n}}\nconsole.log(target());\n"
        );
        println!("SOURCE TypeScript template call\n{source}");
        assert_eq!(run(&source, "js"), "160\n");
        let ws = workspace(&source, "ts");
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            match change_typed(
                &ws, &source, "ts", "80;", true, apply, force, false, "number",
            )
            .await
            {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    println!("ACCEPTED TypeScript apply={apply} force={force}\n{generated}");
                    let executable = generated.replace("limit: number", "limit");
                    assert_eq!(run(&executable, "js"), "160\n");
                    let custom = executable.replace("target(80)", "target(10)");
                    assert_ne!(custom, executable);
                    println!("CUSTOM TypeScript template call\n{custom}");
                    let actual = run(&custom, "js");
                    accepted.push(format!(
                        "{statement}: apply={apply} force={force} custom={actual:?}"
                    ));
                    // Each public operation starts from the exact same source.
                    std::fs::write(ws.path("main.ts"), &source).unwrap();
                }
                Err(error) => {
                    println!("REFUSED TypeScript apply={apply} force={force}: {error}");
                    assert!(error.to_string().contains("incomplete expression coverage"));
                    assert_eq!(ws.read("main.ts"), source);
                }
            }
        }
    }
    assert!(accepted.is_empty(), "{accepted:?}");
}

#[tokio::test]
async fn parenthesized_template_object_division_preserves_single_selection() {
    let mut failures = Vec::new();
    for ext in ["js", "ts"] {
        for template in ["`${({} / 2)}`", "`${(({} / 2))}`", "`${({}) / 2}`"] {
            let source = format!(
                "function target() {{\nconst a = 80;\nconst b = {template};\nreturn a + ':' + b;\n}}\nconsole.log(target());\n"
            );
            println!("SOURCE {ext} object division\n{source}");
            assert_eq!(run(&source, "js"), "80:NaN\n");
            let ws = workspace(&source, ext);
            match change_typed(
                &ws, &source, ext, "80;", false, true, false, false, "number",
            )
            .await
            {
                Ok(plan) => {
                    let generated = ws.read(&format!("main.{ext}"));
                    println!("GENERATED {ext} object division\n{generated}");
                    assert_eq!(plan.replaced, 1);
                    assert!(generated.contains(template));
                    let executable = generated.replace("limit: number", "limit");
                    assert_eq!(run(&executable, "js"), "80:NaN\n");
                    let custom = executable.replace("target(80)", "target(10)");
                    assert_ne!(custom, executable);
                    println!("CUSTOM {ext} object division\n{custom}");
                    assert_eq!(run(&custom, "js"), "10:NaN\n");
                }
                Err(error) => {
                    println!("REFUSED {ext} object division: {error}");
                    failures.push(format!("{ext} {template}: {error}"));
                    assert_eq!(ws.read(&format!("main.{ext}")), source);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn satisfies_after_value_parentheses_still_masks_only_the_type() {
    for value in ["('80')", "String('80')", "({ if: () => '80' }).if()"] {
        let source = format!(
            "function target() {{\nconst a = 80;\nconst b = {value} satisfies `${{80}}`;\nreturn a + 80 + ':' + b;\n}}\nconsole.log(target());\n"
        );
        println!("SOURCE TypeScript value control\n{source}");
        let original = source.replace(" satisfies `${80}`", "");
        assert_eq!(run(&original, "js"), "160:80\n");
        let ws = workspace(&source, "ts");
        let plan = change_typed(
            &ws, &source, "ts", "80;", true, true, false, false, "number",
        )
        .await
        .unwrap();
        let generated = ws.read("main.ts");
        println!("GENERATED TypeScript value control\n{generated}");
        assert_eq!(plan.replaced, 2);
        assert!(generated.contains(&format!("{value} satisfies `${{80}}`")));
        let executable = generated
            .replace(" satisfies `${80}`", "")
            .replace("limit: number", "limit");
        assert_eq!(run(&executable, "js"), "160:80\n");
        let custom = executable.replace("target(80)", "target(10)");
        assert_ne!(custom, executable);
        println!("CUSTOM TypeScript value control (exact type syntax erased)\n{custom}");
        assert_eq!(run(&custom, "js"), "20:80\n");
    }
}

#[tokio::test]
async fn satisfies_after_private_keyword_calls_keeps_type_operands() {
    let mut failures = Vec::new();
    for name in ["if", "while", "for", "with", "switch", "catch"] {
        for (return_type, value, operand) in [("80", "80", "80"), ("'80'", "'80'", "`${80}`")] {
            let source = format!(
                "class C {{\n#{name}(): {return_type} {{ return {value}; }}\ntarget() {{\nconst a = 80;\nconst b = this.#{name}() satisfies {operand};\nreturn a + 80 + ':' + b;\n}}\n}};\nconsole.log(new C().target());\n"
            );
            println!("SOURCE TypeScript private keyword call\n{source}");
            let erase = |text: &str| {
                text.replace(&format!("(): {return_type}"), "()")
                    .replace(&format!(" satisfies {operand}"), "")
                    .replace("limit: number", "limit")
            };
            assert_eq!(run(&erase(&source), "js"), "160:80\n");
            let ws = workspace(&source, "ts");
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let plan = match change_typed(
                    &ws,
                    &source,
                    "ts",
                    "80;\nconst b",
                    true,
                    apply,
                    force,
                    false,
                    "number",
                )
                .await
                {
                    Ok(plan) => plan,
                    Err(error) => {
                        println!(
                            "REFUSED private {name} {operand} apply={apply} force={force}: {error}"
                        );
                        assert_eq!(ws.read("main.ts"), source);
                        failures.push(format!(
                            "{name} {operand} apply={apply} force={force}: {error}"
                        ));
                        continue;
                    }
                };
                let generated = &plan.rewritten[0].1;
                println!(
                    "GENERATED private {name} {operand} apply={apply} force={force} replaced={}\n{generated}",
                    plan.replaced
                );
                assert_eq!(
                    ws.read("main.ts").as_str(),
                    if apply { generated } else { &source }
                );
                // Check the exact type before erasing it for Node: erasure must
                // never hide a replaced literal or a template coverage refusal.
                if plan.replaced != 2
                    || !generated.contains(&format!("this.#{name}() satisfies {operand}"))
                {
                    failures.push(format!(
                        "{name} {operand} apply={apply} force={force}: replaced={} type operand changed",
                        plan.replaced
                    ));
                } else {
                    let executable = erase(generated);
                    assert_eq!(run(&executable, "js"), "160:80\n");
                    let custom = executable.replace(".target(80)", ".target(10)");
                    assert_ne!(custom, executable);
                    println!("CUSTOM private keyword call (exact types erased)\n{custom}");
                    assert_eq!(run(&custom, "js"), "20:80\n");
                }
                std::fs::write(ws.path("main.ts"), &source).unwrap();
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
