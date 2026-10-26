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
use serde_json::json;

#[tokio::test]
async fn c_designator_controls_keep_initializer_values_executable() {
    for (declaration, designated, returned) in [
        (
            "int values[81] = {[ /* index */ 1 ? 80 : 0] = 80};",
            "[ /* index */ 1 ? 80 : 0] = 80",
            "values[80]",
        ),
        (
            "int values[2][81] = {[1][1 ? 80 : 0] = 80};",
            "[1][1 ? 80 : 0] = 80",
            "values[1][80]",
        ),
        (
            "struct {int values[81];} value = {.values[1 ? 80 : 0] = 80};",
            ".values[1 ? 80 : 0] = 80",
            "value.values[80]",
        ),
    ] {
        let source = format!(
            "#include <stdio.h>\nint target(void) {{\nint a=80; {declaration} return a+{returned};\n}}\nint main(void) {{ printf(\"%d\\n\", target()); }}\n"
        );
        assert_eq!(compile_run(&source, "c").as_deref(), Some("160\n"));
        let mut ast = node(&source, designated, "expression", "DesignatedInit");
        let mut value = node(&source, "80};", "expression", "IntegerLiteral");
        value["range"]["end"]["character"] =
            json!(value["range"]["end"]["character"].as_u64().unwrap() - 2);
        ast["children"] = json!([value, node(&source, "1 ? 80 : 0", "expression", "Constant")]);
        let at = if designated.contains("/*") {
            source.find("[ /*").unwrap()
        } else {
            source.find("[1 ?").unwrap()
        };
        // A second bracket in a runtime subscript is also queried. Its real
        // ArraySubscript context must stay executable, unlike the designator.
        let runtime_at = source.rfind("[80]").unwrap();
        let runtime = node(&source, returned, "expression", "ArraySubscript");
        let asts = [(at, ast), (runtime_at, runtime)];
        for (needle, all, count, expected) in [("80;", true, 3, "10\n"), ("80};", false, 1, "90\n")]
        {
            for force in [false, true] {
                let ws = workspace(&source, "c");
                let plan = change_with_ast(
                    &ws,
                    &source,
                    "c",
                    needle,
                    all,
                    true,
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
                    "DESIGNATOR CONTROL force={force} all={all} replaced={}\n{generated}\nCUSTOM\n{custom}",
                    plan.replaced
                );
                assert_eq!(plan.replaced, count);
                assert!(generated.contains("1 ? 80 : 0"));
                assert_eq!(compile_run(generated, "c").as_deref(), Some("160\n"));
                assert_eq!(compile_run(&custom, "c").as_deref(), Some(expected));
            }
        }
    }
}
