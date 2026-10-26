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
async fn go_composite_function_types_keep_executable_initializers() {
    for ty in ["[]func() int", "[1]func() int", "map[int]func() int"] {
        let element = if ty.starts_with("map") {
            "0: factory(x+1)"
        } else {
            "factory(x+1)"
        };
        let source = format!(
            "package main\nimport \"fmt\"\nfunc factory(v int) func() int {{ return func() int {{ return v }} }}\nfunc target(x int) int {{\na := x + 1\nfs := {ty}{{{element}}}\nreturn a+fs[0]()\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go composite\n{source}");
        let original = run(&source, "go");
        let ws = workspace(&source, "go");
        let plan = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.go");
        println!(
            "GENERATED Go composite replaced={}\n{generated}",
            plan.replaced
        );
        let default = run(&generated, "go");
        let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
        println!("CUSTOM Go composite\n{custom}");
        let customized = run(&custom, "go");
        println!("Go original={original:?} default={default:?} customized={customized:?}");
        assert_eq!(default, original);
        assert_eq!(customized, "40\n");
        assert_eq!(plan.replaced, 2);
    }
}

#[tokio::test]
async fn typescript_template_literal_types_are_not_executable() {
    for (alias, tail) in [
        ("type Label = `${80}`;", "return a + 80;"),
        ("type Label = `${80}`", "return a + 80;"),
        ("type Label<T> = `${80}` | T;", "return a + 80;"),
        ("type Label = `${80}`", "a += 80;\nreturn a;"),
        ("type Label = `${80}`\n | 'label'", "a += 80;\nreturn a;"),
        ("type Label = Array<\n `${80}`\n>", "a += 80;\nreturn a;"),
    ] {
        let source = format!(
            "function target() {{\nlet a = 80;\n{alias}\n{tail}\n}}\nconsole.log(target());\n"
        );
        let source = source.as_str();
        println!("SOURCE TypeScript type-only template\n{source}");
        let erased = source.replace(alias, "");
        println!("NODE INPUT (exact type alias erased)\n{erased}");
        assert_eq!(run(&erased, "js"), "160\n");
        let ws = workspace(source, "ts");
        let result =
            change_typed(&ws, source, "ts", "80;", true, true, true, false, "number").await;
        println!("TypeScript extraction={result:?}");
        let plan = result.unwrap();
        let generated = ws.read("main.ts");
        println!("GENERATED TypeScript type-only template\n{generated}");
        assert!(generated.contains(alias));
        assert_eq!(plan.replaced, 2);
        let erased = generated
            .replace(alias, "")
            .replace("limit: number", "limit");
        println!("NODE INPUT (exact alias and generated annotation erased)\n{erased}");
        assert_eq!(run(&erased, "js"), "160\n");
        let custom = erased.replace("target(80)", "target(10)");
        println!("CUSTOM TypeScript type-only template\n{custom}");
        assert_eq!(run(&custom, "js"), "20\n");
    }
}

#[tokio::test]
async fn javascript_interpolation_comments_do_not_nest() {
    for ext in ["js", "ts"] {
        for template in [
            "`${80 /* /* */}`",
            "`${`nested ${80 /* /* */}`.slice(7)}`",
            "`${({value: 80 /* /* */}).value}`",
            "`${/}/.test('}') ? 80 /* /* */ : 0}`",
        ] {
            let source = format!(
                "function target() {{\nconst a = 80;\nconst b = {template};\nreturn a + Number(b);\n}}\nconsole.log(target());\n"
            );
            let source = source.as_str();
            println!("SOURCE {ext} comment interpolation\n{source}");
            assert_eq!(run(source, "js"), "160\n");
            let ws = workspace(source, ext);
            let result =
                change_typed(&ws, source, ext, "80;", false, true, false, false, "number").await;
            println!("{ext} single-selection extraction={result:?}");
            let plan = result.unwrap();
            assert_eq!(plan.replaced, 1);
            let generated = ws.read(&format!("main.{ext}"));
            println!("GENERATED {ext} comment interpolation\n{generated}");
            assert!(generated.contains(template));
            let erased = generated.replace("limit: number", "limit");
            println!("NODE INPUT (generated annotation erased for TS)\n{erased}");
            assert_eq!(run(&erased, "js"), "160\n");
            let custom = erased.replace("target(80)", "target(10)");
            println!("CUSTOM {ext} comment interpolation\n{custom}");
            assert_eq!(run(&custom, "js"), "90\n");
            let ws = workspace(source, ext);
            let error = change_typed(&ws, source, ext, "80;", true, true, true, false, "number")
                .await
                .unwrap_err();
            println!("REFUSED {ext} replace-all: {error}");
            assert!(error.to_string().contains("incomplete expression coverage"));
            assert_eq!(ws.read(&format!("main.{ext}")), source);
        }
    }
}

#[tokio::test]
async fn go_index_before_immediate_function_call_stays_a_literal() {
    let source = "package main\nimport \"fmt\"\nfunc target(x int) int {\na := x + 1\nfs := []int{2}\nb := fs[0] * func(x int) int { return x+1 }(9)\nreturn a+b\n}\nfunc main() { x := 4; fmt.Println(target(x)) }\n";
    println!("SOURCE Go index and literal\n{source}");
    assert_eq!(run(source, "go"), "25\n");
    let ws = workspace(source, "go");
    let plan = change(&ws, source, "go", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    let generated = ws.read("main.go");
    println!("GENERATED Go index and literal\n{generated}");
    assert_eq!(plan.replaced, 1);
    assert_eq!(run(&generated, "go"), "25\n");
    let custom = generated.replace("target(x, x + 1)", "target(x, 20)");
    println!("CUSTOM Go index and literal\n{custom}");
    assert_eq!(run(&custom, "go"), "40\n");
}
