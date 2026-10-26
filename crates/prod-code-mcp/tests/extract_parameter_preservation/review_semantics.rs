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
async fn javascript_selected_method_keys_refuse_without_writes() {
    for key in ["80", "'80'"] {
        let source = format!(
            "function target(){{\nconst o={{{key}(){{return 1;}}}}; return Object.keys(o)[0];\n}}\nconsole.log(target());\n"
        );
        assert_eq!(run(&source, "js"), "80\n");
        let ws = workspace(&source, "js");
        for all in [false, true] {
            let error = change(
                &ws,
                &source,
                "js",
                &format!("select:{key}"),
                all,
                true,
                true,
                false,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("complete executable expression"));
            assert_eq!(ws.read("main.js"), source);
        }
    }
}

#[tokio::test]
async fn javascript_method_keys_and_bindings_preserve_runtime() {
    for (value, method) in [
        ("80", "80(){return 1;}"),
        ("'80'", "'80'(){return 1;}"),
        ("80", "async 80(x=80){return x + 80;}"),
        ("80", "*80(x=80){yield x + 80;}"),
        ("80", "get 80(){return 80;}"),
        ("80", "set 80(x){this.value=x + 80;}"),
    ] {
        let source = format!(
            "function target(){{\nconst a={value}; const o={{{method}}}; return Object.keys(o)[0]+':'+a;\n}}\nconsole.log(target());\n"
        );
        let original = run(&source, "js");
        assert_eq!(original, "80:80\n");
        let ws = workspace(&source, "js");
        let needle = format!("select:{value}");
        let plan = change(&ws, &source, "js", &needle, true, false, false, false)
            .await
            .unwrap();
        let generated = &plan.rewritten[0].1;
        println!("GENERATED method key\n{generated}");
        assert_eq!(run(generated, "js"), original);
        assert_eq!(plan.replaced, 1);
        assert!(generated.contains(method));
        assert_eq!(ws.read("main.js"), source);
        let applied = change(&ws, &source, "js", &needle, true, true, false, false)
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read("main.js"), *generated);
    }
}

#[tokio::test]
async fn javascript_computed_method_keys_remain_executable() {
    let source = "function target(){\nconst a=80; const o={[80](x=80){return x+80;}, [80+1]:80}; return Object.keys(o).join(',')+':'+a+':'+o[80]();\n}\nconsole.log(target());\n";
    let original = run(source, "js");
    let ws = workspace(source, "js");
    let plan = change(&ws, source, "js", "80;", true, false, false, false)
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!("GENERATED computed keys\n{generated}");
    assert_eq!(run(generated, "js"), original);
    assert_eq!(plan.replaced, 5);
    assert!(generated.contains("[limit](x=80){return x+80;}"));
    assert!(generated.contains("[limit+1]:limit"));
    let applied = change(&ws, source, "js", "80;", true, true, false, false)
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read("main.js"), *generated);
}

#[tokio::test]
async fn go_precedence_preserves_expression_identity_and_runtime() {
    for (expression, expected) in [
        ("1+2 << 3", "20\n"),
        ("1+2 >> 1", "5\n"),
        ("1+2 & 3", "6\n"),
        ("1+2 &^ 3", "4\n"),
        ("1+2 | 4", "10\n"),
        ("1+2 ^ 4", "10\n"),
    ] {
        let source = format!(
            "package main\nimport \"fmt\"\nfunc target() int {{\na := 1 + 2; return a + ({expression})\n}}\nfunc main() {{ fmt.Println(target()) }}\n"
        );
        let original = run(&source, "go");
        assert_eq!(original, expected);
        let ws = workspace(&source, "go");
        let plan = change(
            &ws,
            &source,
            "go",
            "select:1 + 2",
            true,
            false,
            false,
            false,
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        println!("GENERATED Go precedence\n{generated}");
        assert_eq!(run(generated, "go"), original);
        let additive =
            expression.contains('|') || expression.contains('^') && !expression.contains("&^");
        assert_eq!(plan.replaced, if additive { 2 } else { 1 });
        if !additive {
            assert!(generated.contains(expression));
        }
        assert_eq!(ws.read("main.go"), source);
        let applied = change(&ws, &source, "go", "select:1 + 2", true, true, false, false)
            .await
            .unwrap();
        assert!(applied.applied);
        assert_eq!(ws.read("main.go"), *generated);
    }
}
