/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[path = "extract_parameter_preservation/aggregate_controls.rs"]
mod aggregate_controls;
#[path = "extract_parameter_preservation/aggregate_defects.rs"]
mod aggregate_defects;
#[path = "extract_parameter_preservation/aggregate_followup.rs"]
mod aggregate_followup;
#[path = "extract_parameter_preservation/aggregate_repair.rs"]
mod aggregate_repair;
#[path = "extract_parameter_preservation/aggregate_review.rs"]
mod aggregate_review;
#[path = "extract_parameter_preservation/array_controls.rs"]
mod array_controls;
#[path = "extract_parameter_preservation/boundaries.rs"]
mod boundaries;
#[path = "extract_parameter_preservation/c_cast_controls.rs"]
mod c_cast_controls;
#[path = "extract_parameter_preservation/c_context_controls.rs"]
mod c_context_controls;
#[path = "extract_parameter_preservation/c_delimiter_controls.rs"]
mod c_delimiter_controls;
#[path = "extract_parameter_preservation/c_designator_controls.rs"]
mod c_designator_controls;
#[path = "extract_parameter_preservation/c_lexer_controls.rs"]
mod c_lexer_controls;
#[path = "extract_parameter_preservation/constant_controls.rs"]
mod constant_controls;
#[path = "extract_parameter_preservation/declaration_controls.rs"]
mod declaration_controls;
#[path = "extract_parameter_preservation/ep1309_controls.rs"]
mod ep1309_controls;
#[path = "extract_parameter_preservation/ep1309_independent.rs"]
mod ep1309_independent;
#[path = "extract_parameter_preservation/final_review.rs"]
mod final_review;
#[path = "extract_parameter_preservation/four_controls.rs"]
mod four_controls;
#[path = "extract_parameter_preservation/four_findings.rs"]
mod four_findings;
#[path = "extract_parameter_preservation/go_reuse.rs"]
mod go_reuse;
#[path = "extract_parameter_preservation/header_alignment.rs"]
mod header_alignment;
#[path = "extract_parameter_preservation/remaining_review.rs"]
mod remaining_review;
#[path = "extract_parameter_preservation/retry_review.rs"]
mod retry_review;
#[path = "extract_parameter_preservation/review_gaps.rs"]
mod review_gaps;
#[path = "extract_parameter_preservation/review_repairs.rs"]
mod review_repairs;
#[path = "extract_parameter_preservation/review_semantics.rs"]
mod review_semantics;
#[path = "extract_parameter_preservation/revision_controls.rs"]
mod revision_controls;
#[path = "extract_parameter_preservation/semantic_boundaries.rs"]
mod semantic_boundaries;
#[path = "extract_parameter_preservation/statement_value_controls.rs"]
mod statement_value_controls;
#[path = "extract_parameter_preservation/storage_controls.rs"]
mod storage_controls;
#[path = "extract_parameter_preservation/support.rs"]
mod support;
#[path = "extract_parameter_preservation/template_controls.rs"]
mod template_controls;
use support::*;

#[tokio::test]
async fn rust_replace_all_preserves_non_expressions() {
    let source = "fn target() -> i32 {\n    let width = 80;\n    let width80 = 7; // 80 stays\n    println!(\"80px {}\", width80);\n    width + 80\n}\nfn main() { println!(\"{}\", target()); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80;", true, false, false, false)
        .await
        .unwrap();
    let generated = &plan.rewritten[0].1;
    println!("GENERATED Rust\n{generated}");
    let observed = run(generated, "rs");
    assert_eq!(observed, original);
    assert_eq!(plan.replaced, 2);
    assert!(generated.contains("let width80 = 7; // 80 stays"));
    assert_eq!(ws.read("main.rs"), source);
    let applied = change(&ws, source, "rs", "80;", true, true, false, false)
        .await
        .unwrap();
    assert!(applied.applied);
    assert_eq!(ws.read("main.rs"), *generated);
}

#[tokio::test]
async fn one_line_body_is_panic_free() {
    let source = "fn target() -> i32 { 80 + 80 }\nfn main() { println!(\"{}\", target()); }\n";
    let original = run(source, "rs");
    let ws = workspace(source, "rs");
    let plan = change(&ws, source, "rs", "80 +", true, true, false, false)
        .await
        .unwrap();
    assert_eq!(plan.replaced, 2);
    assert_eq!(run(&ws.read("main.rs"), "rs"), original);
}

#[tokio::test]
async fn trailing_comma_call_preserves_trivia_and_runtime() {
    for tail in [
        ",\n",
        ", // keep this 80 comment\n",
        " /* before comma */, /* after comma */\n",
        " // before comma\n,\n",
        " /* no trailing comma */\n",
    ] {
        let source = format!(
            "fn target(x: i32) -> i32 {{\n    x + 80\n}}\nfn main() {{ println!(\"{{}}\", target(\n    2{tail})); }}\n"
        );
        let source = source.as_str();
        let original = run(source, "rs");
        let ws = workspace(source, "rs");
        let plan = change(&ws, source, "rs", "80\n", false, true, false, false)
            .await
            .unwrap();
        println!("GENERATED comma\n{}", plan.rewritten[0].1);
        assert_eq!(run(&ws.read("main.rs"), "rs"), original);
        assert!(ws.read("main.rs").contains(tail));
    }
}

#[tokio::test]
async fn javascript_string_and_comment_control() {
    let source = "function target() {\n    const width = 80;\n    const width80 = 7; // 80 stays\n    const regex = /80px/;\n    const larger = 1e+80;\n    const object = {80: '80'};\n    const closure = (x = 80) => x + 80;\n    if (!regex.test('80px') || object[80] !== '80' || closure() !== 160 || larger < 180) throw Error('control');\n    return '80px:' + (width + 80 + width80);\n}\nconsole.log(target());\n";
    let original = run(source, "js");
    let ws = workspace(source, "js");
    let plan = change(&ws, source, "js", "80;", true, true, false, false)
        .await
        .unwrap();
    println!("GENERATED JavaScript\n{}", plan.rewritten[0].1);
    assert_eq!(run(&ws.read("main.js"), "js"), original);
    assert_eq!(plan.replaced, 3);
}
