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
use std::process::Command;

fn cpp_exit(source: &str) -> i32 {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.cpp");
    let binary = dir.path().join("program");
    std::fs::write(&input, source).unwrap();
    let output = Command::new("c++")
        .args(["-std=c++17", "-Wall", "-Wextra"])
        .arg(input)
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap();
    println!(
        "c++: {} {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success());
    let code = Command::new(binary).status().unwrap().code().unwrap();
    println!("C++ program exit: {code}");
    code
}

/// Collect every reported counterexample before failing, so pre-repair evidence is complete.
#[tokio::test]
async fn aggregate_review_counterexamples() {
    let mut failures = Vec::new();
    for (source, needle, argument, expected) in [
        (
            "def target():\n    a = 1 + 2\n    b = 1+2 if True else 0\n    return a + b\nprint(target())\n",
            "select:1 + 2",
            "1 + 2",
            "20\n",
        ),
        (
            "def target():\n    a = 80\n    b = f\"{80}\"\n    return (a, b)\nprint(target())\n",
            "select:80",
            "80",
            "(10, '10')\n",
        ),
    ] {
        println!("SOURCE Python\n{source}");
        let original = run(source, "py");
        let ws = workspace(source, "py");
        let plan = change_typed(&ws, source, "py", needle, true, false, false, false, "int").await;
        match plan {
            Ok(plan) => {
                let generated = &plan.rewritten[0].1;
                println!("GENERATED Python (replaced {})\n{generated}", plan.replaced);
                assert_eq!(run(generated, "py"), original);
                let customized =
                    generated.replace(&format!("print(target({argument}))"), "print(target(10))");
                let actual = run(&customized, "py");
                println!("CUSTOMIZED Python expected {expected:?}, actual {actual:?}");
                let applied =
                    change_typed(&ws, source, "py", needle, true, true, false, false, "int")
                        .await
                        .unwrap();
                assert!(applied.applied);
                assert_eq!(ws.read("main.py"), *generated);
                if actual != expected {
                    failures.push(format!("Python incomplete replacement: {actual:?}"));
                }
            }
            Err(error) => {
                println!("Python coverage refusal: {error}");
                assert!(error.to_string().contains("incomplete expression coverage"));
                for force in [false, true] {
                    let error =
                        change_typed(&ws, source, "py", needle, true, true, force, false, "int")
                            .await
                            .unwrap_err();
                    assert!(error.to_string().contains("incomplete expression coverage"));
                    assert_eq!(ws.read("main.py"), source);
                }
            }
        }
    }

    let source = "int target() { int a = 80; const char *s = R\"(v\"80\")\"; return a + (s[2] == '8'); } int main() { return target(); }\n";
    println!("SOURCE C++\n{source}");
    assert_eq!(cpp_exit(source), 81);
    let ws = workspace(source, "cpp");
    let plan = change_typed(&ws, source, "cpp", "80;", true, false, false, false, "int")
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!("GENERATED C++ (replaced {})\n{generated}", plan.replaced);
    let actual = cpp_exit(generated);
    let applied = change_typed(&ws, source, "cpp", "80;", true, true, false, false, "int")
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read("main.cpp"), *generated);
    if actual != 81 || plan.replaced != 1 {
        failures.push(format!(
            "C++ expected 81, actual {actual}, replaced {}",
            plan.replaced
        ));
    }

    let source = "package main\nimport \"fmt\"\nfunc target(x int) int {\na := x + 1; b := 0; { x := 10; b = x+1; _ = x }; return a + b\n}\nfunc main() { x := 4; fmt.Println(target(x)) }\n";
    println!("SOURCE Go\n{source}");
    assert_eq!(run(source, "go"), "16\n");
    let ws = workspace(source, "go");
    let plan = change(&ws, source, "go", "select:x + 1", true, false, false, false)
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!("GENERATED Go (replaced {})\n{generated}", plan.replaced);
    let actual = run(generated, "go");
    let applied = change(&ws, source, "go", "select:x + 1", true, true, false, false)
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read("main.go"), *generated);
    if actual != "16\n" || plan.replaced != 1 {
        failures.push(format!(
            "Go expected 16, actual {actual:?}, replaced {}",
            plan.replaced
        ));
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
