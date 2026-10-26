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
use prod_code_mcp::extract_parameter::ExtractedParameter;
use prod_code_testkit::Workspace;
use serde_json::{Value, json};

// Scripted AST replies replay the roles and ranges recorded in the separate
// live clangd probe. Compiler and runtime checks execute real C11/C++17 code.
fn cast_ast(source: &str, cast_text: &str, operand_text: &str, kind: &str) -> (usize, Value) {
    let mut ast = node(source, cast_text, "expression", "CXXStaticCast");
    let mut ty = node(source, "<int>", "type", "Builtin");
    ty["range"]["start"]["character"] =
        json!(ty["range"]["start"]["character"].as_u64().unwrap() + 1);
    ty["range"]["end"]["character"] = json!(ty["range"]["end"]["character"].as_u64().unwrap() - 1);
    let start = source.find(cast_text).unwrap() + cast_text.find(operand_text).unwrap();
    let (sl, sc) = position(source, start);
    let (el, ec) = position(source, start + operand_text.len() - 1);
    let operand = json!({"role":"expression","kind":kind,"range":{
        "start":{"line":sl-1,"character":sc-1},
        "end":{"line":el-1,"character":ec-1}}});
    ast["children"] = json!([ty, operand]);
    (source.find("<int>").unwrap(), ast)
}

#[allow(clippy::too_many_arguments)]
async fn call(
    ws: &Workspace,
    source: &str,
    ext: &str,
    needle: &str,
    all: bool,
    apply: bool,
    force: bool,
    asts: &[(usize, Value)],
) -> anyhow::Result<ExtractedParameter> {
    change_with_ast(
        ws,
        source,
        ext,
        needle,
        all,
        apply,
        force,
        false,
        "int",
        &[],
        &[],
        asts,
    )
    .await
}

#[tokio::test]
async fn c_designator_delimiter_selection_refuses_and_retains_positives() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; int values[81] = {[1 ? 80 : 0] = 7}; return a+values[80];\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    assert_eq!(compile_run(source, "c").as_deref(), Some("87\n"));
    let mut ast = node(source, "[1 ? 80 : 0] = 7", "expression", "DesignatedInit");
    ast["children"] = json!([
        node(source, "7", "expression", "IntegerLiteral"),
        node(source, "1 ? 80 : 0", "expression", "Constant")
    ]);
    let asts = [(source.find("[1 ?").unwrap(), ast)];
    let mut failures = Vec::new();
    for all in [false, true] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "c");
            match call(
                &ws,
                source,
                "c",
                "select:[1 ? 80 : 0]",
                all,
                apply,
                force,
                &asts,
            )
            .await
            {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    let custom = generated.replace("target([1 ? 80 : 0])", "target(10)");
                    println!(
                        "PUBLIC designator delimiter all={all} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM\n{custom}",
                        plan.replaced
                    );
                    let _ = compile_run(generated, "c");
                    let _ = compile_run(&custom, "c");
                    failures.push(format!(
                        "designator delimiter accepted all={all} apply={apply} force={force}"
                    ));
                }
                Err(error) => {
                    println!(
                        "REFUSED designator delimiter all={all} apply={apply} force={force}: {error}"
                    );
                    if !error.to_string().contains("complete executable expression")
                        || ws.read("main.c") != source
                    {
                        failures.push(format!(
                            "designator delimiter error/write mismatch all={all} apply={apply} force={force}: {error}"
                        ));
                    }
                }
            }
        }
    }
    for (declaration, designated, selected) in [
        (
            "int values[81] = {0, [1 ? 80 : 0] = 80};",
            "[1 ? 80 : 0] = 80",
            "select:[1 ? 80 : 0]",
        ),
        (
            "int values[2][81] = {[1][1 ? 80 : 0] = 80};",
            "[1][1 ? 80 : 0] = 80",
            "select:[1][1 ? 80 : 0]",
        ),
        (
            "struct {int values[81];} value = {.values[1 ? 80 : 0] = 80};",
            ".values[1 ? 80 : 0] = 80",
            "select:[1 ? 80 : 0]",
        ),
    ] {
        let variant = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {declaration} return a;\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        let mut ast = node(&variant, designated, "expression", "DesignatedInit");
        let mut value = node(&variant, "80};", "expression", "IntegerLiteral");
        value["range"]["end"]["character"] =
            json!(value["range"]["end"]["character"].as_u64().unwrap() - 2);
        ast["children"] = json!([
            value,
            node(&variant, "1 ? 80 : 0", "expression", "Constant")
        ]);
        let asts = [(variant.find("[1 ?").unwrap(), ast)];
        let ws = workspace(&variant, "c");
        if call(&ws, &variant, "c", selected, false, true, true, &asts)
            .await
            .is_ok()
            || ws.read("main.c") != variant
        {
            failures.push(format!("variant accepted or wrote: {declaration}"));
        }
        if declaration.starts_with("int values[81]") {
            let ws = workspace(&variant, "c");
            let init_plan = call(&ws, &variant, "c", "80};", false, true, true, &asts)
                .await
                .unwrap();
            let init_gen = &init_plan.rewritten[0].1;
            let init_custom = init_gen.replace("target(80)", "target(10)");
            println!("PUBLIC C initializer value\n{init_gen}\nCUSTOM\n{init_custom}");
            if init_plan.replaced != 1
                || !init_gen.contains("[1 ? 80 : 0] = limit")
                || compile_run(init_gen, "c").as_deref() != Some("80\n")
                || compile_run(&init_custom, "c").as_deref() != Some("80\n")
            {
                failures.push("C initializer value failed".to_string());
            }
        }
    }
    // Retain runtime subscript and C VLA bound positives under strict C11.
    let ws = workspace(source, "c");
    let subscript = call(&ws, source, "c", "80]", false, true, true, &asts)
        .await
        .unwrap();
    let sub_gen = &subscript.rewritten[0].1;
    let sub_custom = sub_gen.replace("target(80)", "target(0)");
    println!("PUBLIC C runtime subscript\n{sub_gen}\nCUSTOM\n{sub_custom}");
    if subscript.replaced != 1
        || compile_run(sub_gen, "c").as_deref() != Some("87\n")
        || compile_run(&sub_custom, "c").as_deref() != Some("80\n")
    {
        failures.push("C runtime subscript failed".to_string());
    }
    let vla = "#include <stdio.h>\nint target(void) {\nint a=80; int values[1 ? 80 : 1]; return a+sizeof(values)/sizeof(int);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    let vla_ast = (
        vla.find("[1 ?").unwrap(),
        node(vla, "int values[1 ? 80 : 1]", "type", "ConstantArray"),
    );
    let ws = workspace(vla, "c");
    let vla_plan = call(&ws, vla, "c", "80 :", false, true, true, &[vla_ast])
        .await
        .unwrap();
    let vla_gen = &vla_plan.rewritten[0].1;
    let vla_custom = vla_gen.replace("target(80)", "target(10)");
    println!("PUBLIC C VLA bound\n{vla_gen}\nCUSTOM\n{vla_custom}");
    if vla_plan.replaced != 1
        || compile_run(vla_gen, "c").as_deref() != Some("160\n")
        || compile_run(&vla_custom, "c").as_deref() != Some("90\n")
    {
        failures.push("C VLA bound failed".to_string());
    }
    assert!(failures.is_empty(), "{failures:?}");
}

#[tokio::test]
async fn cpp_static_cast_delimiter_selection_refuses_and_retains_positives() {
    let source = "#include <stdio.h>\nint target(void) {\nint a=80; return a+static_cast<int>(80);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    assert_eq!(compile_run(source, "cpp").as_deref(), Some("160\n"));
    let asts = [cast_ast(
        source,
        "static_cast<int>(80)",
        "80)",
        "IntegerLiteral",
    )];
    let mut failures = Vec::new();
    for all in [false, true] {
        for (apply, force) in [(false, false), (true, false), (true, true)] {
            let ws = workspace(source, "cpp");
            match call(&ws, source, "cpp", "select:(80)", all, apply, force, &asts).await {
                Ok(plan) => {
                    let generated = &plan.rewritten[0].1;
                    let custom = generated.replace("target((80))", "target(10)");
                    println!(
                        "PUBLIC cast delimiter all={all} apply={apply} force={force} replaced={}\n{generated}\nCUSTOM\n{custom}",
                        plan.replaced
                    );
                    let _ = compile_run(generated, "cpp");
                    let _ = compile_run(&custom, "cpp");
                    failures.push(format!(
                        "cast delimiter accepted all={all} apply={apply} force={force}"
                    ));
                }
                Err(error) => {
                    println!(
                        "REFUSED cast delimiter all={all} apply={apply} force={force}: {error}"
                    );
                    if !error.to_string().contains("complete executable expression")
                        || ws.read("main.cpp") != source
                    {
                        failures.push(format!(
                            "cast delimiter error/write mismatch all={all} apply={apply} force={force}: {error}"
                        ));
                    }
                }
            }
        }
    }
    // Complete runtime cast extraction remains supported.
    for (apply, force) in [(false, false), (true, false), (true, true)] {
        let ws = workspace(source, "cpp");
        match call(
            &ws,
            source,
            "cpp",
            "select:static_cast<int>(80)",
            true,
            apply,
            force,
            &asts,
        )
        .await
        {
            Ok(plan) => {
                let generated = &plan.rewritten[0].1;
                let custom = generated.replace("target(static_cast<int>(80))", "target(10)");
                println!(
                    "PUBLIC complete cast apply={apply} force={force} replaced={}\n{generated}\nCUSTOM\n{custom}",
                    plan.replaced
                );
                if plan.replaced != 1
                    || compile_run(generated, "cpp").as_deref() != Some("160\n")
                    || compile_run(&custom, "cpp").as_deref() != Some("90\n")
                {
                    failures.push(format!(
                        "complete cast semantics apply={apply} force={force}"
                    ));
                }
            }
            Err(error) => {
                println!("PUBLIC complete cast refused apply={apply} force={force}: {error}");
                failures.push(format!(
                    "complete cast refused apply={apply} force={force}: {error}"
                ));
            }
        }
    }
    // Genuine nested parenthesized operand in static_cast<int>((80)) remains executable,
    // while the outer mandatory cast delimiter pair ((80)) is refused.
    let nested = "#include <stdio.h>\nint target(void) {\nint a=80; return a+static_cast<int>((80));\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    assert_eq!(compile_run(nested, "cpp").as_deref(), Some("160\n"));
    let nested_asts = [cast_ast(nested, "static_cast<int>((80))", "(80))", "Paren")];
    let ws = workspace(nested, "cpp");
    if call(
        &ws,
        nested,
        "cpp",
        "select:((80))",
        false,
        true,
        true,
        &nested_asts,
    )
    .await
    .is_ok()
        || ws.read("main.cpp") != nested
    {
        failures.push("outer cast delimiter ((80)) accepted".to_string());
    }
    for (needle, caller, all, count, expected) in [
        ("select:(80)", "target((80))", false, 1, "90\n"),
        ("80)", "target(80)", false, 1, "90\n"),
        ("80;", "target(80)", true, 2, "20\n"),
    ] {
        let ws = workspace(nested, "cpp");
        match call(&ws, nested, "cpp", needle, all, true, true, &nested_asts).await {
            Ok(plan) => {
                let generated = &plan.rewritten[0].1;
                let custom = generated.replace(caller, "target(10)");
                println!(
                    "PUBLIC nested cast {needle} all={all} replaced={}\n{generated}\nCUSTOM\n{custom}",
                    plan.replaced
                );
                if plan.replaced != count
                    || compile_run(generated, "cpp").as_deref() != Some("160\n")
                    || compile_run(&custom, "cpp").as_deref() != Some(expected)
                {
                    failures.push(format!("nested cast {needle} all={all} semantics failed"));
                }
            }
            Err(error) => failures.push(format!("nested cast {needle} all={all} refused: {error}")),
        }
    }
    let mixed = "#include <stdio.h>\nint target(void) {\nint a=(80); return a+static_cast<int>(80);\n}\nint main(void) { printf(\"%d\\n\", target()); }\n";
    let mixed_asts = [cast_ast(
        mixed,
        "static_cast<int>(80)",
        "80)",
        "IntegerLiteral",
    )];
    let ws = workspace(mixed, "cpp");
    match call(
        &ws,
        mixed,
        "cpp",
        "select:(80)",
        true,
        true,
        true,
        &mixed_asts,
    )
    .await
    {
        Ok(plan) => {
            let generated = &plan.rewritten[0].1;
            let custom = generated.replace("target((80))", "target(10)");
            println!(
                "PUBLIC mixed cast delimiter replace_all replaced={}\n{generated}\nCUSTOM\n{custom}",
                plan.replaced
            );
            if plan.replaced != 1
                || compile_run(generated, "cpp").as_deref() != Some("160\n")
                || compile_run(&custom, "cpp").as_deref() != Some("90\n")
            {
                failures.push(format!("mixed cast replace_all replaced={}", plan.replaced));
            }
        }
        Err(error) => failures.push(format!("mixed cast replace_all refused: {error}")),
    }
    assert!(failures.is_empty(), "{failures:?}");
}
