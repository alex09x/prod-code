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
async fn directional_channel_literals_keep_independent_parameters() {
    let mut failures = Vec::new();
    for ty in ["<-chan int", "chan<- int"] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target(x int) int {{\na := x + 1\ng := func(x int) {ty} {{ fmt.Println(x+1); return nil }}\ng(9)\nreturn a\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go directional channel\n{source}");
        let original = run(&source, "go");
        let ws = workspace(&source, "go");
        let plan = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.go");
        println!(
            "GENERATED Go directional channel replaced={}\n{generated}",
            plan.replaced
        );
        let default = run(&generated, "go");
        let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
        println!("CUSTOM Go directional channel\n{custom}");
        let customized = run(&custom, "go");
        println!("Go original={original:?} default={default:?} customized={customized:?}");
        if default != original || customized != "10\n20\n" || plan.replaced != 1 {
            failures.push((ty, default, customized, plan.replaced));
        }
        let ws = workspace(&source, "go");
        for all in [false, true] {
            let error = change(&ws, &source, "go", "select:x+1", all, true, true, false)
                .await
                .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("not a complete executable expression")
            );
            assert_eq!(ws.read("main.go"), source);
        }
    }
    assert!(
        failures.is_empty(),
        "channel literal failures: {failures:?}"
    );
}

#[tokio::test]
async fn satisfies_template_type_preserves_runtime_replacements() {
    let source = "function target() {\nconst a = 80;\nconst b = '80' satisfies `${80}`;\nreturn a + 80;\n}\nconsole.log(target());\n";
    println!("SOURCE TypeScript satisfies\n{source}");
    let erased = source.replace(" satisfies `${80}`", "");
    println!("NODE INPUT (exact satisfies operand erased)\n{erased}");
    assert_eq!(run(&erased, "js"), "160\n");
    let ws = workspace(source, "ts");
    let result = change_typed(&ws, source, "ts", "80;", true, true, true, false, "number").await;
    println!("TypeScript satisfies extraction={result:?}");
    let plan = result.unwrap();
    let generated = ws.read("main.ts");
    println!("GENERATED TypeScript satisfies\n{generated}");
    assert_eq!(plan.replaced, 2);
    assert!(generated.contains("satisfies `${80}`"));
    let erased = generated
        .replace(" satisfies `${80}`", "")
        .replace("limit: number", "limit");
    println!("NODE INPUT (exact satisfies operand and annotation erased)\n{erased}");
    assert_eq!(run(&erased, "js"), "160\n");
    let custom = erased.replace("target(80)", "target(10)");
    println!("CUSTOM TypeScript satisfies\n{custom}");
    assert_eq!(run(&custom, "js"), "20\n");
}

#[tokio::test]
async fn satisfies_type_boundary_keeps_following_runtime_templates_visible() {
    // A runtime template after the type must still block replace_all, including
    // force. Treating everything up to the semicolon as a type would hide it.
    for expression in [
        "'80' satisfies `${80}` && `${80}`",
        "('80' satisfies (`${80}` | 'label')) && `${80}`",
        "({ satisfies: (x) => x }).satisfies(`${80}`)",
        "satisfies`${80}`",
        "80 satisfies 80 < (Number(`${80}`) > 0 ? 81 : 79)",
    ] {
        let source = format!(
            "function target() {{\nconst a = 80;\nconst satisfies = (parts, x) => x;\nconst b = {expression};\nreturn a + Number(b);\n}}\nconsole.log(target());\n"
        );
        println!("SOURCE TypeScript runtime template boundary\n{source}");
        let erased = source
            .replace(" satisfies `${80}`", "")
            .replace(" satisfies (`${80}` | 'label')", "")
            .replace(" satisfies 80", "");
        println!("NODE INPUT (exact type operands erased)\n{erased}");
        assert_eq!(
            run(&erased, "js"),
            if expression.starts_with("80 satisfies") {
                "81\n"
            } else {
                "160\n"
            }
        );
        let ws = workspace(&source, "ts");
        for force in [false, true] {
            let error = change_typed(
                &ws, &source, "ts", "80;", true, true, force, false, "number",
            )
            .await
            .unwrap_err();
            println!("REFUSED TypeScript runtime template force={force}: {error}");
            assert!(error.to_string().contains("incomplete expression coverage"));
            assert_eq!(ws.read("main.ts"), source);
        }
    }
}

#[tokio::test]
async fn directional_channel_function_types_keep_following_expressions() {
    let source = "package main\nimport \"fmt\"\nfunc target(x int) int {\nvar g func(int) <-chan int\nvar h func(int) chan<- int\n_, _ = g, h\na := x + 1\nb := x+1\nreturn a+b\n}\nfunc main() { x := 4; fmt.Println(target(x)) }\n";
    println!("SOURCE Go directional channel types\n{source}");
    assert_eq!(run(source, "go"), "10\n");
    let ws = workspace(source, "go");
    let plan = change(&ws, source, "go", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    let generated = ws.read("main.go");
    println!("GENERATED Go directional channel types\n{generated}");
    assert_eq!(plan.replaced, 2);
    assert_eq!(run(&generated, "go"), "10\n");
    let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
    println!("CUSTOM Go directional channel types\n{custom}");
    assert_eq!(run(&custom, "go"), "40\n");
}

#[tokio::test]
async fn interpolation_object_division_preserves_single_selection() {
    for ext in ["js", "ts"] {
        let source = "function target() {\nconst a = 80;\nconst b = `${{} / 2}`;\nreturn a;\n}\nconsole.log(target());\n";
        println!("SOURCE {ext} object division\n{source}");
        assert_eq!(run(source, "js"), "80\n");
        let ws = workspace(source, ext);
        let result =
            change_typed(&ws, source, ext, "80;", false, true, false, false, "number").await;
        println!("{ext} object division extraction={result:?}");
        let plan = result.unwrap();
        let generated = ws.read(&format!("main.{ext}"));
        println!("GENERATED {ext} object division\n{generated}");
        assert_eq!(plan.replaced, 1);
        assert!(generated.contains("`${{} / 2}`"));
        let erased = generated.replace("limit: number", "limit");
        println!("NODE INPUT (generated annotation erased for TS)\n{erased}");
        assert_eq!(run(&erased, "js"), "80\n");
        let custom = erased.replace("target(80)", "target(10)");
        println!("CUSTOM {ext} object division\n{custom}");
        assert_eq!(run(&custom, "js"), "10\n");
        let ws = workspace(source, ext);
        let error = change_typed(&ws, source, ext, "80;", true, true, true, false, "number")
            .await
            .unwrap_err();
        println!("REFUSED {ext} object division replace-all: {error}");
        assert!(error.to_string().contains("incomplete expression coverage"));
        assert_eq!(ws.read(&format!("main.{ext}")), source);
    }
}
