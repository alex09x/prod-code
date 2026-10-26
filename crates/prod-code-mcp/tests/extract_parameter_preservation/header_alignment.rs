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
use serde_json::json;

#[tokio::test]
async fn cpp_header_keyword_uses_analyzer_grammar() {
    let source = "#include <stdio.h>\nint target(void) {\nreturn true;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE C++ header\n{source}");
    assert_eq!(run(source, "cpp"), "1\n");
    let at = source.find("true").unwrap();
    // The exact node shape observed from clangd with languageId=c and -xc++.
    let (line, col) = position(source, at);
    let ast = json!({"role":"expression", "kind":"CXXBoolLiteral", "detail":"true",
        "range":{"start":{"line":line-1,"character":col-1},
                 "end":{"line":line-1,"character":col+3}}});
    let mut failures = Vec::new();
    for ext in ["cpp", "h"] {
        for all in [false, true] {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(source, ext);
                match change_with_ast(
                    &ws,
                    source,
                    ext,
                    "select:true",
                    all,
                    apply,
                    force,
                    false,
                    "bool",
                    &[],
                    &[],
                    &[(at, ast.clone())],
                )
                .await
                {
                    Ok(plan) => {
                        let generated = &plan.rewritten[0].1;
                        let custom = generated.replace("target(true)", "target(false)");
                        println!(
                            "GENERATED header {ext} all={all} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM header\n{custom}",
                            plan.replaced
                        );
                        assert_eq!(run(generated, ext), "1\n");
                        assert_eq!(run(&custom, ext), "0\n");
                        assert_eq!(plan.replaced, 1);
                    }
                    Err(e) => {
                        failures.push(format!("{ext} all={all} apply={apply} force={force}: {e}"))
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn c_cpp_keyword_spelling_still_needs_binding_identity() {
    let source = "#include <stdio.h>\nint target(int true) {\nint a=true + 1; int b=0; { int true=10; b=true+1; } return a+b;\n}\nint main(void) { int true=4; printf(\"%d\\n\", target(true)); }\n";
    println!("SOURCE C identifier true\n{source}");
    assert_eq!(run(source, "c"), "16\n");
    let outer = source.find("true)").unwrap();
    let inner = source.find("true=10").unwrap();
    let first = source.find("true + 1").unwrap();
    let second = source.find("true+1").unwrap();
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = workspace(source, "h");
        let plan = change_resolved(
            &ws,
            source,
            "h",
            "select:true + 1",
            true,
            apply,
            force,
            false,
            "int",
            &[(first, outer), (second, inner)],
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(true, true + 1)", "target(true, 20)");
        println!(
            "GENERATED C identifier apply={apply} force={force} replaced={}\n{generated}\nCUSTOM C identifier\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, 1);
        assert_eq!(run(generated, "c"), "16\n");
        assert_eq!(run(&custom, "c"), "31\n");
    }
    for ast in [
        serde_json::Value::Null,
        json!({"role":"expression", "kind":"DeclRef", "detail":"true"}),
    ] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "h");
            let error = change_with_ast(
                &ws,
                source,
                "h",
                "select:true + 1",
                true,
                apply,
                force,
                false,
                "int",
                &[],
                &[],
                &[(first, ast.clone())],
            )
            .await
            .unwrap_err();
            println!("REFUSED unresolved C keyword spelling apply={apply} force={force}: {error}");
            assert!(
                error
                    .to_string()
                    .contains("incomplete C/C++ binding coverage")
            );
            assert_eq!(ws.read("main.h"), source);
        }
    }
}

#[tokio::test]
async fn c_alignas_function_argument_remains_runtime() {
    let source = "#include <stdio.h>\nint seen; void alignas(int n) { seen=n; }\nint target(void) {\nint a=16; alignas(16); return a+seen;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE C runtime alignas\n{source}");
    assert_eq!(run(source, "c"), "32\n");
    for (needle, all, count, custom_output) in [("16;", true, 2, "20\n"), ("16)", false, 1, "26\n")]
    {
        let ws = workspace(source, "c");
        let at = source.find("alignas(16)").unwrap();
        let plan = change_analyzed(
            &ws,
            source,
            "c",
            needle,
            all,
            true,
            false,
            false,
            "int",
            &[],
            &[(at, "### function alignas\n\n```c\nvoid alignas(int n)\n```")],
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(16)", "target(10)");
        println!(
            "GENERATED C alignas all={all} replaced={}\n{generated}\nCUSTOM C alignas\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, count);
        assert_eq!(run(generated, "c"), "32\n");
        assert_eq!(run(&custom, "c"), custom_output);
    }
}

#[tokio::test]
async fn unknown_header_alignment_refuses_before_writes() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=16; alignas(true ? 16 : 0) char s; return a+sizeof(s);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    for (needle, all) in [("16;", true), ("16 :", false)] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "h");
            let error = change_typed(&ws, source, "h", needle, all, apply, force, false, "int")
                .await
                .unwrap_err();
            println!(
                "REFUSED unknown header alignment all={all} apply={apply} force={force}: {error}"
            );
            assert!(
                error
                    .to_string()
                    .contains("incomplete C/C++ alignment coverage")
            );
            assert_eq!(ws.read("main.h"), source);
        }
    }
}

#[tokio::test]
async fn alignment_constants_stay_constant() {
    let mut failures = Vec::new();
    for (ext, declaration) in [
        ("cpp", "alignas(true ? 16 : 0) char s;"),
        ("cpp", "alignas(16) char s;"),
        ("cpp", "alignas((true ? (false ? 0 : 16) : 0)) char s;"),
        ("c", "_Alignas(1 ? 16 : 0) char s;"),
        ("c", "_Alignas(16) char s;"),
    ] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=16; {declaration} return a+sizeof(s);\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        println!("SOURCE alignment {ext}\n{source}");
        assert_eq!(run(&source, ext), "17\n");
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, ext);
            let plan = change_typed(&ws, &source, ext, "16;", true, apply, force, false, "int")
                .await
                .unwrap();
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target(16)", "target(10)");
            println!(
                "GENERATED alignment {ext} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM alignment\n{custom}",
                plan.replaced
            );
            let default = std::panic::catch_unwind(|| run(generated, ext));
            let customized = std::panic::catch_unwind(|| run(&custom, ext));
            if plan.replaced != 1
                || !generated.contains(declaration)
                || !default.as_ref().is_ok_and(|s| s == "17\n")
                || !customized.as_ref().is_ok_and(|s| s == "11\n")
            {
                failures.push(format!(
                    "{ext} {declaration} apply={apply} force={force}: replaced={}",
                    plan.replaced
                ));
            }
        }
        let needle = if declaration.contains("16 :") {
            "16 :"
        } else {
            "16)"
        };
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(&source, ext);
            match change_typed(&ws, &source, ext, needle, false, apply, force, false, "int").await {
                Err(e) => {
                    println!("REFUSED selected alignment {ext} apply={apply} force={force}: {e}");
                    assert_eq!(ws.read(&format!("main.{ext}")), source);
                }
                Ok(_) => failures.push(format!(
                    "selected alignment {ext} {declaration} apply={apply} force={force} accepted"
                )),
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
