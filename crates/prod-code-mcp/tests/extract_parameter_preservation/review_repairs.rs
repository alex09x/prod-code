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

async fn preserve(source: &str, ext: &str, selected: &str, count: usize, expected: &str) {
    println!("ORIGINAL {ext}\n{source}");
    let original = run(source, ext);
    assert_eq!(original, expected);
    let ws = workspace(source, ext);
    let needle = format!("select:{selected}");
    let plan = change_typed(&ws, source, ext, &needle, true, false, false, false, "int")
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!(
        "PLANNED {ext} ({} replacements)\n{generated}",
        plan.replaced
    );
    assert_eq!(ws.read(&format!("main.{ext}")), source);
    let applied = change_typed(&ws, source, ext, &needle, true, true, false, false, "int")
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read(&format!("main.{ext}")), *generated);
    let observed = run(generated, ext);
    println!("RESULT {ext}: before={original:?} after={observed:?}");
    assert_eq!(observed, original);
    assert_eq!(plan.replaced, count);
}

#[tokio::test]
async fn cpp_not_preserves_operand_precedence() {
    preserve(
        "#include <iostream>\nint target() { int a = 1 + 2; int b = not 1 + 2; return a + b; }\nint main() { std::cout << target() << '\\n'; }\n",
        "cpp", "1 + 2", 1, "5\n",
    ).await;
}

#[tokio::test]
async fn python_blank_line_keeps_nested_scope_excluded() {
    preserve(
        "def target():\n    a = 80\n    def inner(limit):\n\n        return 80\n    return a + inner(1)\nprint(target())\n",
        "py", "80", 1, "160\n",
    ).await;
}

#[tokio::test]
async fn cpp_complete_not_expressions_still_match() {
    preserve(
        "#include <iostream>\nint target() { int a = not 1; int b = not    1; int c = not 1 + 2; return a + b + c; }\nint main() { std::cout << target() << '\\n'; }\n",
        "cpp", "not 1", 3, "2\n",
    ).await;
}

#[tokio::test]
async fn python_trivia_lines_and_real_dedent() {
    for trivia in ["    \n", "# comment\n", "\n# comment\n    \n"] {
        let source = format!(
            "def target():\n    a = 80\n    def inner(limit):\n{trivia}        return 80\n    return a + 80 + inner(1)\nprint(target())\n"
        );
        preserve(&source, "py", "80", 2, "240\n").await;
        preserve(&source.replace('\n', "\r\n"), "py", "80", 2, "240\n").await;
    }
}
