/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::path::Path;
use std::process::Command;

fn line_col(text: &str, offset: usize) -> (u32, u32) {
    let before = &text[..offset];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32 + 1;
    let column = before
        .rsplit('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .count() as u32
        + 1;
    (line, column)
}

async fn extract(
    main: &str,
    other: &str,
) -> (Workspace, prod_code_mcp::extract_function::Extracted) {
    let workspace = Workspace::new(&[("main.ts", main), ("other.ts", other)]);
    workspace.commit();
    let file = workspace.path("main.ts");
    let selected = "rate * 10";
    let start = main.find(selected).expect("selection exists");
    let end = start + selected.len();
    let (line, column) = line_col(main, start);
    let (end_line, end_column) = line_col(main, end);
    let gateway = ScriptedGateway::start(|method, _| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await;
    let result = prod_code_mcp::extract_function_polyglot::extract_function_polyglot(
        gateway.addr(),
        &workspace.root(),
        &file,
        (line, column),
        (end_line, end_column),
        "scale",
        true,
        true,
        true,
    )
    .await
    .expect("the helper extraction is generated");
    (workspace, result)
}

fn rewritten<'a>(
    result: &'a prod_code_mcp::extract_function::Extracted,
    filename: &str,
) -> &'a str {
    result
        .rewritten
        .iter()
        .find(|(path, _)| Path::new(path).ends_with(filename))
        .map(|(_, text)| text.as_str())
        .expect("file is included in the returned edits")
}

fn evaluate_helper(workspace: &Workspace, main: &str, calls: &str) -> String {
    std::fs::write(workspace.root().join("main.ts"), main).unwrap();
    std::fs::write(
        workspace.root().join("probe.mjs"),
        format!("import {{ scale }} from './main.ts';\nconsole.log({calls});\n"),
    )
    .unwrap();
    let output = Command::new("node")
        .args(["--experimental-strip-types", "probe.mjs"])
        .current_dir(workspace.root())
        .output()
        .expect("Node is available on the build node");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

#[tokio::test]
async fn in_file_and_cross_file_literals_all_reach_the_helper() {
    let main = "export function invoice(rate: number) {\n    const first = rate * 10;\n    const second = rate * 20;\n    return first + second;\n}\n";
    let other = "export function quote(rate: number) {\n    return rate * 25;\n}\n";
    let (workspace, result) = extract(main, other).await;
    let main = rewritten(&result, "main.ts");
    let other = rewritten(&result, "other.ts");

    assert!(main.contains("return rate * value"), "{main}");
    assert!(main.contains("scale(rate, 10)"), "{main}");
    assert!(main.contains("scale(rate, 20)"), "{main}");
    assert!(other.contains("scale(rate, 25)"), "{other}");
    assert_eq!(
        evaluate_helper(&workspace, main, "scale(2, 10), scale(2, 20), scale(3, 25)"),
        "20 40 75"
    );
}

#[tokio::test]
async fn a_cross_file_only_literal_difference_is_parameterized() {
    let main = "export function invoice(rate: number) {\n    return rate * 10;\n}\n";
    let other = "export function quote(rate: number) {\n    return rate * 25;\n}\n";
    let (workspace, result) = extract(main, other).await;
    let main = rewritten(&result, "main.ts");
    let other = rewritten(&result, "other.ts");

    assert!(main.contains("return rate * value"), "{main}");
    assert!(main.contains("scale(rate, 10)"), "{main}");
    assert!(other.contains("scale(rate, 25)"), "{other}");
    assert_eq!(
        evaluate_helper(&workspace, main, "scale(2, 10), scale(3, 25)"),
        "20 75"
    );
}
