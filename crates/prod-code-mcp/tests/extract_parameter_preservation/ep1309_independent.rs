/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::{
    c_context_controls::{compile_run, node},
    support::*,
};
use serde_json::Value;

// Public extract() uses scripted semantic replies, including the CXXConstruct
// shape separately observed with live clangd. Compilers and runtimes are real.
#[tokio::test]
async fn ep1309_independent_selections_beside_unsupported_casts() {
    for (source, cast_text, selected, all_modes, original, changed) in [
        (
            "struct Box { explicit Box(int) {} };\nint target() {\nauto box=static_cast<Box>(1);\nreturn 42;\n}\nint main(){return target()!=42;}\n",
            "static_cast<Box>(1)",
            "42",
            true,
            "",
            "",
        ),
        (
            "#include <stdio.h>\ntemplate<int N> struct Box { Box(int v):value(v){} int value; };\nint target(void) {\nint a=80; return a+static_cast<Box<true ? 80 : 1>>(80).value;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n",
            "static_cast<Box<true ? 80 : 1>>(80)",
            "80",
            false,
            "160\n",
            "87\n",
        ),
        (
            "int target() {\n#if 0\nauto box=static_cast<Box>(1);\n#endif\nreturn 42;\n}\nint main(){return target()!=42;}\n",
            "",
            "42",
            true,
            "",
            "",
        ),
        (
            "#include <stdio.h>\nstruct Box { explicit Box(int v):value(v){} int value; };\nint target() {\nreturn static_cast<Box>(1).value+42;\n}\nint main(){printf(\"%d\\n\",target());}\n",
            "static_cast<Box>(1)",
            "42",
            true,
            "43\n",
            "8\n",
        ),
    ] {
        println!("ORIGINAL independent cast\n{source}");
        assert_eq!(compile_run(source, "cpp").as_deref(), Some(original));
        let at = source.find("static_cast").unwrap() + "static_cast".len();
        let ast = if cast_text.is_empty() {
            Value::Null
        } else {
            node(source, cast_text, "expression", "CXXConstruct")
        };
        for all in [false, true].into_iter().filter(|all| !all || all_modes) {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(source, "cpp");
                let plan = change_with_ast(
                    &ws,
                    source,
                    "cpp",
                    &format!("select:{selected}"),
                    all,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[],
                    &[(at, ast.clone())],
                )
                .await
                .unwrap();
                let generated = &plan.rewritten[0].1;
                let custom = generated
                    .replace(&format!("target({selected})"), "target(7)")
                    .replace("!=42", "!=7");
                println!(
                    "PUBLIC independent all={all} apply={apply} force={force}\n{generated}\nCUSTOM\n{custom}"
                );
                assert_eq!(plan.replaced, 1);
                if !cast_text.is_empty() {
                    assert!(generated.contains(cast_text));
                }
                assert_eq!(compile_run(generated, "cpp").as_deref(), Some(original));
                assert_eq!(compile_run(&custom, "cpp").as_deref(), Some(changed));
                assert_eq!(
                    ws.read("main.cpp").as_str(),
                    if apply { generated } else { source }
                );
            }
        }
    }
}

#[tokio::test]
async fn ep1309_unsupported_cast_dependencies_still_refuse() {
    let source = "#include <stdio.h>\ntemplate<int N> struct Box { Box(int v):value(v){} int value; };\nint target(void) {\nint a=80; return a+static_cast<Box<true ? 80 : 1>>(80).value;\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    let cast_text = "static_cast<Box<true ? 80 : 1>>(80)";
    let at = source.find("static_cast").unwrap() + "static_cast".len();
    for ast in [
        node(source, cast_text, "expression", "CXXConstruct"),
        Value::Null,
    ] {
        for (selected, all) in [("80", true), (cast_text, false), ("(80)", false)] {
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(source, "cpp");
                let error = change_with_ast(
                    &ws,
                    source,
                    "cpp",
                    &format!("select:{selected}"),
                    all,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[],
                    &[(at, ast.clone())],
                )
                .await
                .unwrap_err();
                println!(
                    "REFUSED unsupported cast selected={selected} all={all} apply={apply} force={force}: {error}"
                );
                assert_eq!(ws.read("main.cpp"), source);
            }
        }
    }
}
