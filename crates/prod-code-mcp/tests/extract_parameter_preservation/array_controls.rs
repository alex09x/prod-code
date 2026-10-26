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
use serde_json::{Value, json};
use std::process::Command;

// Minimal scripted clangd nodes using the bracket-query roles, kinds and ranges
// observed in the retained live probe. Symbols/diagnostics remain simulated.
fn context(source: &str, selected: &str, kind: &str, role: &str) -> (usize, Value) {
    let at = source.find(selected).unwrap();
    let end = at + source[at..].find(']').unwrap() + 1;
    let start = source[..at].rfind("int ").unwrap();
    let (sl, sc) = position(source, start);
    let (el, ec) = position(source, end);
    (
        at,
        json!({"role":role, "kind":kind,
        "range":{"start":{"line":sl-1,"character":sc-1},
                 "end":{"line":el-1,"character":ec-1}}}),
    )
}

fn strict_cpp(source: &str) -> bool {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("main.cpp");
    std::fs::write(&input, source).unwrap();
    let output = Command::new("c++")
        .args(["-std=c++17", "-pedantic-errors"])
        .arg(&input)
        .arg("-o")
        .arg(dir.path().join("program"))
        .output()
        .unwrap();
    println!(
        "strict C++17: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    output.status.success()
}

#[tokio::test]
async fn cpp_array_bounds_remain_constant() {
    let mut failures = Vec::new();
    for (declaration, selected, length) in [
        (
            "int values[true ? 80 : 1];",
            "[true",
            "sizeof(values)/sizeof(values[0])",
        ),
        (
            "int values[(true ? 80 : 1)];",
            "[(true",
            "sizeof(values)/sizeof(values[0])",
        ),
        (
            "int values[2][true ? 80 : 1];",
            "[true",
            "sizeof(values)/sizeof(values[0][0])/2",
        ),
        (
            "using Values = int[true ? 80 : 1]; Values values;",
            "[true",
            "sizeof(values)/sizeof(values[0])",
        ),
        (
            "int (*values)[true ? 80 : 1];",
            "[true",
            "sizeof(*values)/sizeof((*values)[0])",
        ),
    ] {
        for ext in ["cpp", "h"] {
            let source = format!(
                "#include <stdio.h>\nint target(void) {{\nint a=80; {declaration} return a+{length};\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
            );
            println!("SOURCE C++ array\n{source}");
            assert!(strict_cpp(&source));
            assert_eq!(run(&source, "cpp"), "160\n");
            let ast = context(&source, selected, "ConstantArray", "type");
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, ext);
                let plan = change_with_ast(
                    &ws,
                    &source,
                    ext,
                    "80;",
                    true,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[],
                    &[ast.clone()],
                )
                .await
                .unwrap();
                let generated = &plan.rewritten[0].1;
                let custom = generated.replace("target(80)", "target(10)");
                assert_ne!(&custom, generated);
                println!(
                    "GENERATED C++ array apply={apply} force={force} replaced={}\n{generated}\nCUSTOM C++ array\n{custom}",
                    plan.replaced
                );
                let strict = strict_cpp(generated) & strict_cpp(&custom);
                let default = run(generated, "cpp");
                let customized = run(&custom, "cpp");
                if !strict
                    || plan.replaced != 1
                    || !generated.contains(declaration)
                    || default != "160\n"
                    || customized != "90\n"
                {
                    failures.push(format!("{declaration} apply={apply} force={force}: replaced={} default={default:?} custom={customized:?}", plan.replaced));
                }
            }
            for (apply, force) in [(false, false), (true, false), (true, true)] {
                let ws = workspace(&source, ext);
                match change_with_ast(
                    &ws,
                    &source,
                    ext,
                    "80 :",
                    false,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[],
                    &[ast.clone()],
                )
                .await
                {
                    Err(e) => println!("REFUSED C++ array apply={apply} force={force}: {e}"),
                    Ok(_) => failures.push(format!(
                        "{declaration}: bound selected apply={apply} force={force}"
                    )),
                }
                assert_eq!(ws.read(&format!("main.{ext}")), source);
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn runtime_index_and_c_vla_bounds_remain_executable() {
    for ext in ["c", "cpp"] {
        let source = "#include <stdio.h>\nint target(void) {\nint a=80; int values[81]={0}; values[80]=80; return a+values[1 ? 80 : 0];\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
        println!("SOURCE runtime index {ext}\n{source}");
        assert_eq!(run(source, ext), "160\n");
        let asts = [
            context(source, "[80]", "ArraySubscript", "expression"),
            context(source, "[1 ?", "ArraySubscript", "expression"),
        ];
        for (needle, all, count, expected) in [("80;", true, 4, "20\n"), ("80 :", false, 1, "80\n")]
        {
            let ws = workspace(source, ext);
            let plan = change_with_ast(
                &ws,
                source,
                ext,
                needle,
                all,
                true,
                false,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await
            .unwrap();
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target(80)", "target(10)");
            println!(
                "GENERATED runtime index {ext} replaced={}\n{generated}\nCUSTOM runtime index\n{custom}",
                plan.replaced
            );
            assert_eq!(plan.replaced, count);
            assert_eq!(run(generated, ext), "160\n");
            assert_eq!(run(&custom, ext), expected);
        }
    }
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; int values[1 ? 80 : 1]; return a+sizeof(values)/sizeof(values[0]);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE C variable-length array\n{source}");
    assert_eq!(run(source, "c"), "160\n");
    for (needle, all, count, expected) in [("80;", true, 2, "20\n"), ("80 :", false, 1, "90\n")] {
        let ws = workspace(source, "c");
        let plan = change_typed(&ws, source, "c", needle, all, true, false, false, "int")
            .await
            .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(80)", "target(10)");
        println!(
            "GENERATED C VLA all={all} replaced={}\n{generated}\nCUSTOM C VLA\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, count);
        assert_eq!(run(generated, "c"), "160\n");
        assert_eq!(run(&custom, "c"), expected);
    }
}

#[tokio::test]
async fn overloaded_cpp_subscript_remains_executable() {
    let source = "#include <stdio.h>\nstruct Values { int operator[](int n) { return n; } };\nint target(void) {\nint a=80; Values values; return a+values[true ? 80 : 1];\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    println!("SOURCE overloaded subscript\n{source}");
    assert_eq!(run(source, "cpp"), "160\n");
    let (at, mut ast) = context(source, "[true", "CXXOperatorCall", "expression");
    ast["children"] = json!([{"role":"expression", "kind":"ImplicitCast", "children":[
        {"role":"expression", "kind":"DeclRef", "detail":"operator[]"}]}]);
    for (needle, all, count, expected) in [("80;", true, 2, "20\n"), ("80 :", false, 1, "90\n")] {
        let ws = workspace(source, "cpp");
        let plan = change_with_ast(
            &ws,
            source,
            "cpp",
            needle,
            all,
            true,
            false,
            false,
            "int",
            &[],
            &[],
            &[(at, ast.clone())],
        )
        .await
        .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(80)", "target(10)");
        println!(
            "GENERATED overloaded index all={all} replaced={}\n{generated}\nCUSTOM overloaded index\n{custom}",
            plan.replaced
        );
        assert_eq!(plan.replaced, count);
        assert_eq!(run(generated, "cpp"), "160\n");
        assert_eq!(run(&custom, "cpp"), expected);
    }
}

#[tokio::test]
async fn unknown_cpp_bracket_context_refuses_before_writes() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; int values[true ? 80 : 1]; return a+sizeof(values)/sizeof(values[0]);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        for needle in ["80;", "80 :"] {
            for asts in [
                vec![],
                vec![(
                    source.find("[true").unwrap(),
                    json!({"role":"type", "kind":"ConstantArray"}),
                )],
                vec![context(source, "[true", "VariableArray", "type")],
            ] {
                let ws = workspace(source, "cpp");
                let e = change_with_ast(
                    &ws,
                    source,
                    "cpp",
                    needle,
                    true,
                    apply,
                    force,
                    false,
                    "int",
                    &[],
                    &[],
                    &asts,
                )
                .await
                .unwrap_err();
                println!("REFUSED unknown bracket apply={apply} force={force}: {e}");
                assert!(e.to_string().contains("incomplete C/C++ array coverage"));
                assert_eq!(ws.read("main.cpp"), source);
            }
        }
    }
}
