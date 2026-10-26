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
async fn go_named_results_and_var_locals_keep_reused_identity() {
    let mut failures = Vec::new();
    for (label, signature, declaration, finish) in [
        (
            "named result",
            "(seed int) (x int)",
            "x = seed",
            "return a+b",
        ),
        ("var local", "(seed int) int", "var x = seed", "return a+b"),
        (
            "typed var local",
            "(seed int) int",
            "var x int = seed",
            "return a+b",
        ),
        (
            "zero var local",
            "(seed int) int",
            "var x int\nx = seed",
            "return a+b",
        ),
        (
            "grouped results",
            "(seed int) (x, z int)",
            "x = seed",
            "return a+b, 0",
        ),
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target{signature} {{\n{declaration}\na := x + 1\nx, y := x, 0\nb := x+1\n_ = y\n{finish}\n}}\nfunc main() {{ x := 4; fmt.Println(target(x)) }}\n"
        );
        println!("SOURCE {label}\n{source}");
        let expected = if label == "grouped results" {
            "10 0\n"
        } else {
            "10\n"
        };
        assert_eq!(run(&source, "go"), expected);
        let ws = workspace(&source, "go");
        let plan = change(
            &ws,
            &source,
            "go",
            "select:x + 1",
            true,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        println!("GENERATED {label} replaced={}\n{generated}", plan.replaced);
        assert_eq!(run(generated, "go"), expected);
        let customized = generated.replace("target(x, x + 1)", "target(x, 10)");
        assert_ne!(customized, *generated);
        let actual = run(&customized, "go");
        let expected = if label == "grouped results" {
            "20 0\n"
        } else {
            "20\n"
        };
        println!("CUSTOMIZED {label} expected={expected:?} actual={actual:?}");
        assert_eq!(ws.read("main.go"), source);
        let applied = change(&ws, &source, "go", "select:x + 1", true, true, false, false)
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read("main.go"), *generated);
        if plan.replaced != 2 || actual != expected {
            failures.push(format!(
                "{label}: replaced={}, customized={actual:?}",
                plan.replaced
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn go_aggregate_binding_sources_keep_reused_identity() {
    let mut failures = Vec::new();
    for (label, extra, signature, declaration, expression, caller, expected) in [
        (
            "grouped var",
            "",
            "target(seed int) int",
            "var (x = seed)",
            "x + 1",
            "target(x)",
            "20\n",
        ),
        (
            "grouped var specs",
            "",
            "target(seed int) int",
            "var (\n z int = seed\n x = z\n)\n_ = z",
            "x + 1",
            "target(x)",
            "20\n",
        ),
        (
            "receiver",
            "type Number int\n",
            "(x Number) target() int",
            "",
            "int(x) + 1",
            "x.target()",
            "20\n",
        ),
        (
            "parenthesized parameter",
            "",
            "target(x (int)) int",
            "",
            "x + 1",
            "target(x)",
            "20\n",
        ),
        (
            "grouped parenthesized parameters",
            "",
            "target(x, z (int)) int",
            "_ = z",
            "x + 1",
            "target(x, x)",
            "20\n",
        ),
        (
            "parenthesized result",
            "",
            "target(seed int) (x (int))",
            "x = seed",
            "x + 1",
            "target(x)",
            "20\n",
        ),
        (
            "grouped parenthesized results",
            "",
            "target(seed int) (x, z (int))",
            "x = seed",
            "x + 1",
            "target(x)",
            "20 0\n",
        ),
    ] {
        let finish = if label == "grouped parenthesized results" {
            "return a+b, 0"
        } else {
            "return a+b"
        };
        let value = if label == "receiver" {
            "Number(4)"
        } else {
            "4"
        };
        let source = format!(
            "package main\nimport \"fmt\"\n{extra}func {signature} {{\n{declaration}\na := {expression}\nx, y := x, 0\nb := {}\n_ = y\n{finish}\n}}\nfunc main() {{ x := {value}; fmt.Println({caller}) }}\n",
            expression.replace(' ', "")
        );
        println!("SOURCE {label}\n{source}");
        let original = expected.replace("20", "10");
        assert_eq!(run(&source, "go"), original);
        let ws = workspace(&source, "go");
        let needle = format!("select:{expression}");
        let plan = change(&ws, &source, "go", &needle, true, false, false, false)
            .await
            .unwrap();
        let generated = &plan.rewritten[0].1;
        println!("GENERATED {label} replaced={}\n{generated}", plan.replaced);
        assert_eq!(run(generated, "go"), original);
        let call = if caller == "x.target()" {
            format!("x.target({expression})")
        } else {
            format!("{}, {expression})", caller.trim_end_matches(')'))
        };
        let customized = generated.replace(&call, &call.replace(expression, "10"));
        assert_ne!(customized, *generated);
        let actual = run(&customized, "go");
        println!("CUSTOMIZED {label} expected={expected:?} actual={actual:?}");
        assert_eq!(ws.read("main.go"), source);
        let applied = change(&ws, &source, "go", &needle, true, true, false, false)
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read("main.go"), *generated);
        if plan.replaced != 2 || actual != expected {
            failures.push(format!(
                "{label}: replaced={}, customized={actual:?}",
                plan.replaced
            ));
        }
        // The same supported declaration must keep single-selection extraction available.
        let ws = workspace(&source, "go");
        let single = change(&ws, &source, "go", &needle, false, true, false, false)
            .await
            .unwrap();
        assert_eq!(single.replaced, 1);
        let customized = ws
            .read("main.go")
            .replace(&call, &call.replace(expression, "10"));
        assert_eq!(run(&customized, "go"), expected.replace("20", "15"));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

#[tokio::test]
async fn go_grouped_var_specs_keep_initializer_and_block_scope() {
    let source = "package main\nimport \"fmt\"\nfunc target(x int) int {\na := x + 1\nb := 0\n{\nvar (\n z = x+1\n x = z\n)\nb = x+1\n_ = x\n}\nreturn a+b\n}\nfunc main() { x := 4; fmt.Println(target(x)) }\n";
    assert_eq!(run(source, "go"), "11\n");
    let ws = workspace(source, "go");
    let plan = change(&ws, source, "go", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    let generated = ws.read("main.go");
    println!("GENERATED grouped var scope\n{generated}");
    assert_eq!(plan.replaced, 2);
    assert!(generated.contains("z = limit\n x = z"));
    assert!(generated.contains("b = x+1"));
    assert_eq!(run(&generated, "go"), "11\n");
    let customized = generated.replace("target(x, x + 1)", "target(x, 10)");
    assert_eq!(run(&customized, "go"), "21\n");
}
