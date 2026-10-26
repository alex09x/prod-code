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
async fn go_function_literal_keeps_its_own_parameter_scope() {
    let source = "package main\nimport \"fmt\"\nfunc target(x int) int {\na := x + 1\ng := func(x int) int { return x+1 }\nreturn a+g(9)\n}\nfunc main() { x := 4; fmt.Println(target(x)) }\n";
    println!("SOURCE Go literal\n{source}");
    let original = run(source, "go");
    let ws = workspace(source, "go");
    let plan = change(&ws, source, "go", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    let generated = ws.read("main.go");
    println!(
        "GENERATED Go literal replaced={}\n{generated}",
        plan.replaced
    );
    let default = run(&generated, "go");
    let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
    println!("CUSTOM Go literal\n{custom}");
    let customized = run(&custom, "go");
    println!("Go original={original:?} default={default:?} customized={customized:?}");
    assert_eq!(default, original);
    assert_eq!(customized, "30\n");
    assert_eq!(plan.replaced, 1);
    assert!(generated.contains("func(x int) int { return x+1 }"));
    for (all, force) in [(true, true), (false, false)] {
        let ws = workspace(source, "go");
        let plan = change(&ws, source, "go", "select:x + 1", all, true, force, false)
            .await
            .unwrap();
        assert_eq!(plan.replaced, 1);
        assert_eq!(ws.read("main.go"), generated);
    }
    // The outer parameter cannot be introduced into an independently bound body.
    let ws = workspace(source, "go");
    for all in [false, true] {
        let error = change(&ws, source, "go", "select:x+1", all, true, true, false)
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

#[tokio::test]
async fn javascript_interpolation_refuses_incomplete_replace_all() {
    let source = "function target() {\nconst a = 80;\nconst b = `${80}`;\nreturn a + Number(b);\n}\nconsole.log(target());\n";
    println!("SOURCE JavaScript interpolation\n{source}");
    let original = run(source, "js");
    let mut accepted = Vec::new();
    let ws = workspace(source, "js");
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        match change(&ws, source, "js", "80;", true, apply, force, false).await {
            Ok(plan) => {
                let generated = &plan.rewritten[0].1;
                println!("GENERATED JavaScript apply={apply} force={force}\n{generated}");
                let default = run(generated, "js");
                let custom = generated.replace("target(80)", "target(10)");
                println!("CUSTOM JavaScript\n{custom}");
                let customized = run(&custom, "js");
                println!(
                    "JavaScript original={original:?} default={default:?} customized={customized:?}"
                );
                accepted.push((apply, force, plan.replaced));
            }
            Err(error) => {
                println!("REFUSED JavaScript apply={apply} force={force}: {error}");
                assert!(error.to_string().contains("incomplete expression coverage"));
                assert_eq!(ws.read("main.js"), source);
            }
        }
    }
    assert!(
        accepted.is_empty(),
        "incomplete plans accepted: {accepted:?}"
    );
}

#[tokio::test]
async fn go_literal_results_and_function_types_preserve_outer_extraction() {
    for (declaration, call) in [
        (
            "g := func(x int) (result int) { result = x+1; return }",
            "g(9)",
        ),
        (
            "g := func(x int) struct{ value int } { return struct{ value int }{x+1} }",
            "g(9).value",
        ),
        (
            "g := func(x int) interface{ Value() int } { _ = x+1; return nil }",
            "10",
        ),
        ("var g func(x int) int", "10"),
        ("g := (func(x int) int)(nil)", "10"),
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target(x int) int {{\n{declaration}\n_ = g\na := x + 1\nb := x+1\nreturn a+b+{call}\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go signature control\n{source}");
        assert_eq!(run(&source, "go"), "20\n");
        let ws = workspace(&source, "go");
        let plan = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.go");
        println!("GENERATED Go signature control\n{generated}");
        assert_eq!(plan.replaced, 2);
        assert!(generated.contains(declaration));
        assert_eq!(run(&generated, "go"), "20\n");
        let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
        println!("CUSTOM Go signature control\n{custom}");
        assert_eq!(run(&custom, "go"), "50\n");
    }
}

#[tokio::test]
async fn template_boundaries_preserve_single_selection_and_static_data() {
    for ext in ["js", "ts"] {
        for template in [
            "`${80}`",
            "`${`nested ${80}`}`",
            "`${/}/.test('}') ? 80 : 0}`",
        ] {
            let source = format!(
                "function target() {{\nconst a = 80;\nconst b = {template};\nreturn a + ':' + b;\n}}\nconsole.log(target());\n"
            );
            println!("SOURCE {ext} template control\n{source}");
            // Node executes the common JS/TS syntax subset. For emitted TS below,
            // erase only the generated parameter annotation, not arbitrary types.
            let original = run(&source, "js");
            // Refused operations must leave this same fixture reusable. Avoid a
            // new synchronized workspace (and watcher) for each no-write check.
            let ws = workspace(&source, ext);
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let error = change_typed(
                    &ws, &source, ext, "80;", true, apply, force, false, "number",
                )
                .await
                .unwrap_err();
                println!("REFUSED {ext} template apply={apply} force={force}: {error}");
                assert!(error.to_string().contains("incomplete expression coverage"));
                assert_eq!(ws.read(&format!("main.{ext}")), source);
            }
            let plan = change_typed(
                &ws, &source, ext, "80;", false, true, false, false, "number",
            )
            .await
            .unwrap();
            let generated = ws.read(&format!("main.{ext}"));
            println!("GENERATED {ext} single selection\n{generated}");
            assert_eq!(plan.replaced, 1);
            assert!(generated.contains(template));
            let generated = generated.replace("limit: number", "limit");
            println!("NODE INPUT (parameter annotation erased for TS)\n{generated}");
            assert_eq!(run(&generated, "js"), original);
            let custom = generated.replace("target(80)", "target(10)");
            println!("CUSTOM {ext} single selection\n{custom}");
            assert_eq!(run(&custom, "js"), original.replacen("80:", "10:", 1));
        }
        let source = "function target() {\nconst a = 80;\nconst plain = `80`;\nconst escaped = `\\${80}`;\nconst quoted = '${80}'; // `${80}` is comment data\nif (plain !== '80' || escaped !== quoted) throw Error('data');\nreturn a + 80;\n}\nconsole.log(target());\n";
        let ws = workspace(source, ext);
        let plan = change_typed(&ws, source, ext, "80;", true, true, false, false, "number")
            .await
            .unwrap();
        let generated = ws.read(&format!("main.{ext}"));
        println!("GENERATED {ext} static templates\n{generated}");
        assert_eq!(plan.replaced, 2);
        assert!(generated.contains("const plain = `80`;"));
        assert!(generated.contains("const escaped = `\\${80}`;"));
        let generated = generated.replace("limit: number", "limit");
        println!("NODE INPUT (parameter annotation erased for TS)\n{generated}");
        assert_eq!(run(&generated, "js"), "160\n");
        let custom = generated.replace("target(80)", "target(10)");
        println!("CUSTOM {ext} static templates\n{custom}");
        assert_eq!(run(&custom, "js"), "20\n");
    }
}
