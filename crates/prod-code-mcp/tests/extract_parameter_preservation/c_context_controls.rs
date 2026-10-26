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

// Scripted semantic answers reproduce the roles and ranges measured by the
// separate real-clangd probe. Public extract() and compilers run independently.
pub(super) fn node(source: &str, marker: &str, role: &str, kind: &str) -> Value {
    let start = source.find(marker).unwrap();
    let (sl, sc) = position(source, start);
    let (el, ec) = position(source, start + marker.len());
    json!({"role":role,"kind":kind,"range":{
        "start":{"line":sl-1,"character":sc-1},
        "end":{"line":el-1,"character":ec-1}}})
}

pub(super) fn compile_run(source: &str, ext: &str) -> Option<String> {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join(format!("main.{ext}"));
    let bin = dir.path().join("program");
    std::fs::write(&input, source).unwrap();
    let output = Command::new(if ext == "c" { "cc" } else { "c++" })
        .args([
            if ext == "c" { "-std=c11" } else { "-std=c++17" },
            "-pedantic-errors",
        ])
        .arg(input)
        .arg("-o")
        .arg(&bin)
        .output()
        .unwrap();
    println!(
        "COMPILER {ext}: {}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    if !output.status.success() {
        return None;
    }
    let output = Command::new(bin).output().unwrap();
    assert!(output.status.success());
    let value = String::from_utf8(output.stdout).unwrap();
    println!("RUNTIME {value:?}");
    Some(value)
}

fn positions(source: &str, needle: &str) {
    println!("SOURCE\n{source}");
    for (at, _) in source.match_indices("target") {
        println!(
            "SEMANTIC target position 1-based {:?}",
            position(source, at)
        );
    }
    let at = source.find(needle).unwrap();
    println!(
        "SELECTION 1-based {:?}..{:?}",
        position(source, at),
        position(source, at + 2)
    );
}

#[tokio::test]
async fn c_designator_constants_are_preserved() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; int values[81] = {[1 ? 80 : 0] = 7}; return a+values[80];\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    positions(source, "80;");
    assert_eq!(compile_run(source, "c").as_deref(), Some("87\n"));
    let mut ast = node(source, "[1 ? 80 : 0] = 7", "expression", "DesignatedInit");
    ast["children"] = json!([
        node(source, "7", "expression", "IntegerLiteral"),
        node(source, "1 ? 80 : 0", "expression", "Constant")
    ]);
    let asts = [(source.find("[1 ?").unwrap(), ast)];
    let mut failures = Vec::new();
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = workspace(source, "c");
        let plan = change_with_ast(
            &ws,
            source,
            "c",
            "80;",
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
        .unwrap();
        let generated = &plan.rewritten[0].1;
        let custom = generated.replace("target(80)", "target(10)");
        println!(
            "PUBLIC designator apply={apply} force={force} replaced={}\n{generated}\nCUSTOM\n{custom}",
            plan.replaced
        );
        let default = compile_run(generated, "c");
        let customized = compile_run(&custom, "c");
        if plan.replaced != 2
            || !generated.contains("[1 ? 80 : 0] = 7")
            || default.as_deref() != Some("87\n")
            || customized.as_deref() != Some("10\n")
        {
            failures.push(format!(
                "designator apply={apply} force={force} replaced={}",
                plan.replaced
            ));
        }
        let ws = workspace(source, "c");
        positions(source, "80 :");
        let result = change_with_ast(
            &ws,
            source,
            "c",
            "80 :",
            false,
            apply,
            force,
            false,
            "int",
            &[],
            &[],
            &asts,
        )
        .await;
        println!("PUBLIC direct designator apply={apply} force={force}: {result:?}");
        if result.is_ok() || ws.read("main.c") != source {
            failures.push(format!(
                "direct designator apply={apply} force={force} wrote or accepted"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn cpp_static_cast_operands_remain_executable() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; return a+static_cast<int>(80);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    assert_eq!(compile_run(source, "cpp").as_deref(), Some("160\n"));
    let mut ast = node(
        source,
        "static_cast<int>(80)",
        "expression",
        "CXXStaticCast",
    );
    // The type child must be the cast's int, not the function return type.
    let mut ty = node(source, "<int>", "type", "Builtin");
    ty["range"]["start"]["character"] =
        json!(ty["range"]["start"]["character"].as_u64().unwrap() + 1);
    ty["range"]["end"]["character"] = json!(ty["range"]["end"]["character"].as_u64().unwrap() - 1);
    let mut operand = node(source, "80)", "expression", "IntegerLiteral");
    operand["range"]["end"]["character"] =
        json!(operand["range"]["end"]["character"].as_u64().unwrap() - 1);
    ast["children"] = json!([ty, operand]);
    let asts = [(source.find("<int>").unwrap(), ast)];
    let mut failures = Vec::new();
    for (needle, all, count, expected) in [("80)", false, 1, "90\n"), ("80;", true, 2, "20\n")] {
        positions(source, needle);
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "cpp");
            match change_with_ast(
                &ws,
                source,
                "cpp",
                needle,
                all,
                apply,
                force,
                false,
                "int",
                &[],
                &[],
                &asts,
            )
            .await
            {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    let custom = generated.replace("target(80)", "target(10)");
                    println!(
                        "PUBLIC cast all={all} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM\n{custom}",
                        plan.replaced
                    );
                    let default = compile_run(generated, "cpp");
                    let customized = compile_run(&custom, "cpp");
                    if plan.replaced != count
                        || default.as_deref() != Some("160\n")
                        || customized.as_deref() != Some(expected)
                    {
                        failures.push(format!("cast all={all} changed semantics"));
                    }
                }
                Err(error) => {
                    println!("PUBLIC cast refused: {error}");
                    failures.push(format!(
                        "cast all={all} apply={apply} force={force}: {error}"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
