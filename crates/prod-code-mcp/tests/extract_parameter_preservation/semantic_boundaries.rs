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

async fn preserve(source: &str, ext: &str, selected: &str, ty: &str, count: usize) {
    println!("ORIGINAL {ext}\n{source}");
    let original = run(source, ext);
    let ws = workspace(source, ext);
    let needle = format!("select:{selected}");
    let plan = change_typed(&ws, source, ext, &needle, true, false, false, false, ty)
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!(
        "PLANNED {ext} ({} replacements)\n{generated}",
        plan.replaced
    );
    assert_eq!(ws.read(&format!("main.{ext}")), source);
    let applied = change_typed(&ws, source, ext, &needle, true, true, false, false, ty)
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
async fn python_logical_statement_boundaries() {
    preserve(
        "def target():\n    a = 1 + 2\n    b = 1\n    +2\n    return a + b\nprint(target())\n",
        "py",
        "1 + 2",
        "int",
        1,
    )
    .await;
}

#[tokio::test]
async fn rust_bitwise_comparison_precedence() {
    preserve("fn target() -> bool { let a = false == false; let b = false&false==false; a && b }\nfn main() { println!(\"{}\", target()); }\n",
        "rs", "false == false", "bool", 1).await;
}

#[tokio::test]
async fn python_continuations_remain_equivalent() {
    preserve("def target():\n    a = 1 + 2\n    b = 1 \\\n        +2\n    c = (1\n         +2)\n    d = (1 # keep comment\n         +2)\n    return a + b + c + d\nprint(target())\n",
        "py", "1 + 2", "int", 3).await;
}

#[tokio::test]
async fn python_multiline_call_argument_context() {
    for args in ["\n    2,\n", "\n    2\n", "\n    2, # keep comment\n"] {
        let source = format!("def target(x):\n    return x + 80\nprint(target({args}))\n");
        preserve(&source, "py", "80", "int", 1).await;
    }
    for args in ["\n", "\n    # keep comment\n", "\r\n    \r\n"] {
        let source = format!("def target():\n    return 80\nprint(target({args}))\n");
        preserve(&source, "py", "80", "int", 1).await;
    }
}

#[tokio::test]
async fn python_operator_nodes_follow_python_grammar() {
    for (selected, other, ty, count) in [
        ("False == False", "False&False==False", "bool", 1),
        ("1 < 2", "0<1<2", "bool", 1),
        ("1 == 1", "0<1==1", "bool", 1),
        ("0 < 1 < 2", "0<1<2", "bool", 2),
        ("1 not in [2]", "1  not  in [2]", "bool", 2),
        ("1 is not None", "1 is  not None", "bool", 2),
        ("not 1", "not 1 == False", "bool", 1),
        ("-2", "-2**2", "int", 1),
        ("True and False", "True and False and True", "bool", 1),
    ] {
        let source = format!(
            "def target():\n    a = {selected}\n    b = {other}\n    return (a, b)\nprint(target())\n"
        );
        preserve(&source, "py", selected, ty, count).await;
    }
}

#[tokio::test]
async fn python_chain_fragments_and_unsupported_grammar_refuse_without_writes() {
    for (expression, selected) in [
        ("0 < 1 < 2", "0 < 1"),
        ("0 < 1 < 2", "1 < 2"),
        ("0 < 1 == 1", "1 == 1"),
        ("(lambda: 1 + 2)()", "1 + 2"),
        ("'1' '2'", "'1'"),
    ] {
        let source = format!("def target():\n    return {expression}\nprint(target())\n");
        run(&source, "py");
        let ws = workspace(&source, "py");
        let needle = format!("select:{selected}");
        for apply in [false, true] {
            let error = change_typed(
                &ws, &source, "py", &needle, true, apply, true, false, "bool",
            )
            .await
            .unwrap_err();
            println!("REFUSED {expression:?}, selection={selected:?}: {error}");
            assert!(error.to_string().contains("complete executable expression"));
            assert_eq!(ws.read("main.py"), source);
        }
    }
}
