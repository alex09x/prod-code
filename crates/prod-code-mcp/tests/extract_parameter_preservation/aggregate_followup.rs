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
async fn cpp_raw_suffix_is_part_of_the_complete_expression() {
    let mut failures = Vec::new();
    for (prefix, character) in [
        ("R", "char"),
        ("u8R", "char"),
        ("uR", "char16_t"),
        ("UR", "char32_t"),
        ("LR", "wchar_t"),
    ] {
        let raw = format!("{prefix}\"tag(v)tag\"");
        let literal = format!("{raw}_n");
        let source = format!(
            "#include <cstddef>\n#include <iostream>\nint operator \"\" _n(const {character}*, std::size_t) {{ return 80; }}\nint limit_n = 7;\nint target() {{\nreturn {literal};\n}}\nint main() {{ std::cout << target() << '\\n'; }}\n"
        );
        println!("SOURCE C++ suffix\n{source}");
        assert_eq!(run(&source, "cpp"), "80\n");
        // Both selection modes must reject the partial token, even when forced.
        for all in [false, true] {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, "cpp");
                let result = change_typed(
                    &ws,
                    &source,
                    "cpp",
                    &format!("select:{raw}"),
                    all,
                    apply,
                    force,
                    false,
                    &format!("const {character}*"),
                )
                .await;
                match result {
                    Ok(plan) => {
                        let generated = &plan.rewritten[0].1;
                        let actual = run(generated, "cpp");
                        println!("PARTIAL C++ generated\n{generated}OUTPUT {actual:?}");
                        failures.push(format!("{prefix}: partial raw token accepted ({actual:?})"));
                    }
                    Err(error) => {
                        println!("C++ partial refusal: {error}");
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
        // Whole user-defined literals remain extractable, and equivalent occurrences
        // include the suffix rather than joining the new name to an identifier.
        let source = source.replace(
            &format!("return {literal};"),
            &format!("return {literal} + {literal};"),
        );
        let ws = workspace(&source, "cpp");
        let result = change_typed(
            &ws,
            &source,
            "cpp",
            &format!("select:{literal}"),
            true,
            true,
            false,
            false,
            "int",
        )
        .await;
        match result {
            Ok(plan) => {
                println!("WHOLE C++ generated\n{}", plan.rewritten[0].1);
                assert_eq!(run(&ws.read("main.cpp"), "cpp"), "160\n");
                assert_eq!(plan.replaced, 2);
                assert_eq!(
                    run(
                        &ws.read("main.cpp")
                            .replace(&format!("target({literal})"), "target(10)"),
                        "cpp"
                    ),
                    "20\n"
                );
            }
            Err(error) => failures.push(format!("{prefix}: whole literal refused: {error}")),
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn go_continued_short_declaration_preserves_all_bindings() {
    let mut failures = Vec::new();
    for head in [
        "x,\n y",
        "x, /* continuation\n */ y",
        "x, // continuation\n y",
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target(x int) int {{\na := x + 1; b := 0; {{ {head} := 10, 0; b = x+1; _ = x; _ = y }}; return a+b\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go continuation\n{source}");
        assert_eq!(run(&source, "go"), "16\n");
        let ws = workspace(&source, "go");
        let plan = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.go");
        println!("GENERATED Go continuation\n{generated}");
        let actual = run(&generated, "go");
        if actual != "16\n" || plan.replaced != 1 {
            failures.push(format!(
                "{head:?}: output {actual:?}, replaced {}",
                plan.replaced
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn go_multiline_control_headers_refuse_before_writes() {
    let mut failures = Vec::new();
    for header in [
        "if\nx := 10; x > 0 { a += x+1; _ = x }",
        "for\nx := 10; x < 11; x++ { a += x+1; _ = x }",
        "switch\nx := 10; x { case 10: a += x+1; _ = x }",
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target(x int) int {{\na := x + 1; {header}; return a\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE Go header\n{source}");
        assert_eq!(run(&source, "go"), "16\n");
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, "go");
            match change(
                &ws,
                &source,
                "go",
                "select:x + 1",
                true,
                apply,
                force,
                false,
            )
            .await
            {
                Ok(plan) => {
                    println!("UNEXPECTED Go header plan\n{}", plan.rewritten[0].1);
                    let actual = run(&plan.rewritten[0].1, "go");
                    failures.push(format!("{header:?}: accepted, output {actual:?}"));
                }
                Err(error) => {
                    println!("Go coverage refusal: {error}");
                    assert!(error.to_string().contains("incomplete Go binding coverage"));
                    assert_eq!(ws.read("main.go"), source);
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
