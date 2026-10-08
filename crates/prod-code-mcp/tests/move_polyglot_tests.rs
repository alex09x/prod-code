/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::move_item::move_item;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
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
async fn test_move_typescript_function_across_files() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/calc.ts",
            r#"export function add(a: number, b: number): number {
    return a + b;
}

export function compute(x: number): number {
    return add(x, 10);
}
"#,
        ),
        (
            "src/main.ts",
            r#"import { add } from "./calc";

export function run() {
    return add(1, 2);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("src/calc.ts");
    let target_file = root.join("src/helpers.ts");
    let main_file = root.join("src/main.ts");
    let gw = fake_gateway().await;

    let res = move_item(
        gw.addr(),
        &root,
        &calc_file,
        1,
        17,
        &target_file,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "add");
    assert!(res.applied);
    assert!(res.created.is_some());

    let helpers_content = fs::read_to_string(&target_file).unwrap();
    assert!(helpers_content.contains("export function add(a: number, b: number): number"));

    let calc_content = fs::read_to_string(&calc_file).unwrap();
    assert!(!calc_content.contains("function add(a: number, b: number)"));
    assert!(calc_content.contains("import { add } from \"./helpers\";"));
    assert!(calc_content.contains("return add(x, 10);"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("import { add } from \"./helpers\";"));
}

#[tokio::test]
async fn test_move_typescript_multi_import_narrowing() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/utils.ts",
            r#"export function add(a: number, b: number): number {
    return a + b;
}

export function multiply(a: number, b: number): number {
    return a * b;
}
"#,
        ),
        (
            "src/caller.ts",
            r#"import { add, multiply } from "./utils";

export function test() {
    return add(2, 3) + multiply(4, 5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let utils_file = root.join("src/utils.ts");
    let math_file = root.join("src/math.ts");
    let caller_file = root.join("src/caller.ts");
    let gw = fake_gateway().await;

    let res = move_item(
        gw.addr(),
        &root,
        &utils_file,
        1,
        17,
        &math_file,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "add");
    assert!(res.applied);

    let caller_content = fs::read_to_string(&caller_file).unwrap();
    assert!(caller_content.contains("import { multiply } from \"./utils\";"));
    assert!(caller_content.contains("import { add } from \"./math\";"));
}

#[tokio::test]
async fn test_move_python_function_and_callers() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "pkg/utils.py",
            r#"def helper(x):
    return x * 2

def run(x):
    return helper(x) + 1
"#,
        ),
        (
            "pkg/main.py",
            r#"from pkg.utils import helper

def execute():
    return helper(42)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let utils_file = root.join("pkg/utils.py");
    let ops_file = root.join("pkg/ops.py");
    let main_file = root.join("pkg/main.py");
    let gw = fake_gateway().await;

    let res = move_item(gw.addr(), &root, &utils_file, 1, 5, &ops_file, true, false)
        .await
        .unwrap();

    assert_eq!(res.symbol, "helper");
    assert!(res.applied);

    let ops_content = fs::read_to_string(&ops_file).unwrap();
    assert!(ops_content.contains("def helper(x):"));

    let utils_content = fs::read_to_string(&utils_file).unwrap();
    assert!(!utils_content.contains("def helper(x):"));
    assert!(utils_content.contains("from pkg.ops import helper"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("from pkg.ops import helper"));
}

#[tokio::test]
async fn test_move_python_decorated_class() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "pkg/models.py",
            r#"@dataclass
class Item:
    name: str
    price: float

class Order:
    item: Item
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let models_file = root.join("pkg/models.py");
    let item_file = root.join("pkg/item.py");
    let gw = fake_gateway().await;

    let res = move_item(
        gw.addr(),
        &root,
        &models_file,
        2,
        7,
        &item_file,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "Item");
    assert!(res.applied);

    let item_content = fs::read_to_string(&item_file).unwrap();
    assert!(item_content.contains(
        "@dataclass
class Item:"
    ));

    let models_content = fs::read_to_string(&models_file).unwrap();
    assert!(!models_content.contains("class Item:"));
    assert!(models_content.contains("from pkg.item import Item"));
}

#[tokio::test]
async fn test_move_go_same_package() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc/calc.go",
            r#"package calc

func Add(a, b int) int {
	return a + b
}

func Compute(x int) int {
	return Add(x, 10)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc/calc.go");
    let math_file = root.join("calc/math.go");
    let gw = fake_gateway().await;

    let res = move_item(gw.addr(), &root, &calc_file, 3, 6, &math_file, true, false)
        .await
        .unwrap();

    assert_eq!(res.symbol, "Add");
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("package calc"));
    assert!(math_content.contains("func Add(a, b int) int"));

    let calc_content = fs::read_to_string(&calc_file).unwrap();
    assert!(!calc_content.contains("func Add(a, b int) int"));
    assert!(calc_content.contains("func Compute(x int) int"));
}

#[tokio::test]
async fn test_move_cpp_function() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/calc.cpp",
            r#"int add(int a, int b) {
    return a + b;
}

int compute(int x) {
    return add(x, 10);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("src/calc.cpp");
    let helpers_file = root.join("src/helpers.hpp");
    let gw = fake_gateway().await;

    let res = move_item(
        gw.addr(),
        &root,
        &calc_file,
        1,
        5,
        &helpers_file,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "add");
    assert!(res.applied);

    let helpers_content = fs::read_to_string(&helpers_file).unwrap();
    assert!(helpers_content.contains("int add(int a, int b)"));
}

#[tokio::test]
async fn test_move_swift_function_with_carried_imports() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "Sources/App/Utils.swift",
            r#"import Foundation

public func add(a: Int, b: Int) -> Int {
    return a + b
}

public func compute(x: Int) -> Int {
    return add(a: x, b: 10)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let utils_file = root.join("Sources/App/Utils.swift");
    let math_file = root.join("Sources/App/Math.swift");
    let gw = fake_gateway().await;

    let res = move_item(
        gw.addr(),
        &root,
        &utils_file,
        3,
        13,
        &math_file,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "add");
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("import Foundation"));
    assert!(math_content.contains("public func add(a: Int, b: Int) -> Int"));
}

#[tokio::test]
async fn test_move_collision_rejected() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/a.ts",
            r#"export function foo() { return 1; }
"#,
        ),
        (
            "src/b.ts",
            r#"export function foo() { return 2; }
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let a_file = root.join("src/a.ts");
    let b_file = root.join("src/b.ts");
    let gw = fake_gateway().await;

    let err = move_item(gw.addr(), &root, &a_file, 1, 17, &b_file, false, false)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("already declares `foo`"));
}

#[tokio::test]
async fn test_move_go_method_rejected() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.go",
            r#"package main

type Calc struct{}

func (c *Calc) Add(a, b int) int {
	return a + b
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.go");
    let target_file = root.join("other.go");
    let gw = fake_gateway().await;

    let err = move_item(
        gw.addr(),
        &root,
        &calc_file,
        5,
        16,
        &target_file,
        false,
        false,
    )
    .await
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("is a method with a receiver; move it with `code_move_method`")
    );
}
