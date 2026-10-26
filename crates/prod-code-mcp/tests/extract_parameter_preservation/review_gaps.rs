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
async fn regex_at_statement_boundary_stays_opaque() {
    let source = "function target(){ const width=80; /80/.test('80') || console.log('changed'); return width; }\nconsole.log(target());\n";
    let original = run(source, "js");
    let ws = workspace(source, "js");
    let plan = change(&ws, source, "js", "80;", true, false, false, false)
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!("GENERATED statement regex\n{generated}");
    assert_eq!(run(generated, "js"), original);
    assert_eq!(plan.replaced, 1);
    assert!(generated.contains("/80/.test('80')"));
    assert_eq!(ws.read("main.js"), source);
    let applied = change(&ws, source, "js", "80;", true, true, false, false)
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read("main.js"), *generated);
}

#[tokio::test]
async fn multiline_comment_argument_insertion_stays_executable() {
    for (ext, tail) in [
        ("rs", " /* keep\ncomment */"),
        ("rs", ", /* keep\ncomment */"),
        ("rs", " /* keep\ncomment */   "),
        ("js", " /* keep\ncomment */"),
        ("js", ", /* keep\ncomment */"),
    ] {
        let source = if ext == "rs" {
            format!(
                "fn target(x: i32) -> i32 {{ x + 80 }}\nfn main() {{ println!(\"{{}}\", target(2{tail})); }}\n"
            )
        } else {
            format!("function target(x) {{ return x + 80; }}\nconsole.log(target(2{tail}));\n")
        };
        let original = run(&source, ext);
        let ws = workspace(&source, ext);
        let needle = if ext == "rs" { "80 }" } else { "80;" };
        let plan = change(&ws, &source, ext, needle, false, false, false, false)
            .await
            .unwrap();
        let generated = &plan.rewritten[0].1;
        println!("GENERATED multiline comment\n{generated}");
        assert_eq!(run(generated, ext), original);
        assert!(generated.contains(tail));
        assert_eq!(ws.read(&format!("main.{ext}")), source);
        let applied = change(&ws, &source, ext, needle, false, true, false, false)
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read(&format!("main.{ext}")), *generated);
    }
}

#[tokio::test]
async fn regex_after_control_and_division_after_values_stay_distinct() {
    for body in [
        "if (width) /80/.test('80') || console.log('changed'); return width;",
        "{ /[8/0]+/.test('80') || console.log('changed'); } return width;",
        "return width / 80 + (width) / 80;",
        "return width++ / 80 + ({return: width}).return / 80;",
        "return ({if: () => width}).if() / 80 + width / 80;",
    ] {
        let source =
            format!("function target() {{\nlet width = 80; {body}\n}}\nconsole.log(target());\n");
        let original = run(&source, "js");
        let ws = workspace(&source, "js");
        let plan = change(&ws, &source, "js", "80;", true, true, false, false)
            .await
            .unwrap();
        let generated = ws.read("main.js");
        println!("GENERATED lexical context\n{generated}");
        assert_eq!(run(&generated, "js"), original);
        assert_eq!(plan.replaced, if body.contains(" / 80") { 3 } else { 1 });
        if body.contains(".test") {
            assert!(generated.contains(body));
        }
    }
}

#[tokio::test]
async fn ambiguous_brace_slash_refuses_without_writes_even_with_force() {
    let source = "function target() {\nconst width = 80; if (width) {} /80/.test('80'); return width;\n}\nconsole.log(target());\n";
    assert_eq!(run(source, "js"), "80\n");
    let ws = workspace(source, "js");
    for force in [false, true] {
        let error = change(&ws, source, "js", "80;", true, true, force, false)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("ambiguous JavaScript slash"));
        assert_eq!(ws.read("main.js"), source);
    }
}
