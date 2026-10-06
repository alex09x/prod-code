/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_function::extract_function;
use prod_code_testkit::{answers, ScriptedGateway, Workspace};
use std::fs;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_extract_function_typescript_expression_and_duplicates() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export function compute(a: number, b: number): number {
    const res1 = (a + b) * 2;
    const res2 = (a + b) * 2;
    return res1 + res2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.ts");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 18),
        (2, 29),
        "calcTotal",
        true,
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "calcTotal");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("function calcTotal("));
    assert!(updated.contains("const res1 = calcTotal(a, b);"));
    assert!(updated.contains("const res2 = calcTotal(a, b);"));
}

#[tokio::test]
async fn test_extract_function_python_indentation() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "app.py",
            r#"def process(x, y):
    val = x * 2 + y
    other = x * 2 + y
    return val + other
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("app.py");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 11),
        (2, 20),
        "calc_val",
        true,
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "calc_val");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("def calc_val(x, y):"));
    assert!(updated.contains("val = calc_val(x, y)"));
    assert!(updated.contains("other = calc_val(x, y)"));
}

#[tokio::test]
async fn test_extract_function_go() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "main.go",
            r#"package main

func Run(a int, b int) int {
	res1 := a * 10 + b
	res2 := a * 10 + b
	return res1 + res2
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("main.go");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (4, 10),
        (4, 20),
        "calc",
        true,
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "calc");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("func calc(a int, b int) int"));
    assert!(updated.contains("res1 := calc(a, b)"));
    assert!(updated.contains("res2 := calc(a, b)"));
}

#[tokio::test]
async fn test_extract_function_cpp() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.cpp",
            r#"int calculate(int x, int y) {
    int v1 = x * 3 + y;
    int v2 = x * 3 + y;
    return v1 + v2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("math.cpp");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 14),
        (2, 23),
        "multiply_add",
        true,
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "multiply_add");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("multiply_add(int x, int y)"));
    assert!(updated.contains("v1 = multiply_add(x, y);"));
    assert!(updated.contains("v2 = multiply_add(x, y);"));
}

#[tokio::test]
async fn test_extract_function_swift() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "Helper.swift",
            r#"func score(hits: Int, bonus: Int) -> Int {
    let s1 = hits * 5 + bonus
    let s2 = hits * 5 + bonus
    return s1 + s2
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("Helper.swift");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 14),
        (2, 30),
        "calcScore",
        true,
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "calcScore");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("func calcScore("));
    assert!(updated.contains("calcScore("));
}

#[tokio::test]
async fn test_extract_function_parameterize_duplicates() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export function handle(rate: number): number {
    const fee1 = rate * 10;
    const fee2 = rate * 25;
    return fee1 + fee2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 18),
        (2, 27),
        "computeFee",
        true,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "computeFee");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("computeFee(rate, 10)"));
    assert!(updated.contains("computeFee(rate, 25)"));
}

#[tokio::test]
async fn test_extract_function_other_files_workspace_wide() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/calc.ts",
            r#"export function doA(x: number, y: number): number {
    return (x + y) * 10;
}
"#,
        ),
        (
            "src/other.ts",
            r#"export function doB(x: number, y: number): number {
    return (x + y) * 10;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("src/calc.ts");
    let other_file = root.join("src/other.ts");
    let gw = fake_gateway().await;

    let mut extracted = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 12),
        (2, 24),
        "scaleSum",
        true,
        false,
        true,
    )
    .await
    .unwrap();

    assert_eq!(extracted.name, "scaleSum");
    assert_eq!(extracted.replaced(), 1);
    extracted.write(false).unwrap();

    let updated_other = fs::read_to_string(&other_file).unwrap();
    assert!(updated_other.contains("scaleSum(x, y)"));
    assert!(updated_other.contains("import { scaleSum } from \"./calc\";"));
}

#[tokio::test]
async fn test_extract_function_name_collision_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "app.ts",
            r#"export function existing() {}
export function run(a: number) {
    const v = a + 1;
    return v;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("app.ts");
    let gw = fake_gateway().await;

    let res = extract_function(
        gw.addr(),
        &root,
        &file,
        (3, 15),
        (3, 20),
        "existing",
        true,
        false,
        false,
    )
    .await;

    assert!(res.is_err());
    let err = res.err().unwrap().to_string();
    assert!(err.contains("already has something called `existing`"));
}

#[tokio::test]
async fn test_extract_function_unbalanced_bracket_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "app.ts",
            r#"export function run(a: number, b: number) {
    const v = calculate(a, b);
    return v;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("app.ts");
    let gw = fake_gateway().await;

    let res = extract_function(
        gw.addr(),
        &root,
        &file,
        (2, 15),
        (2, 27),
        "subCall",
        true,
        false,
        false,
    )
    .await;

    assert!(res.is_err());
    let err = res.err().unwrap().to_string();
    assert!(err.contains("unbalanced delimiter"));
}
