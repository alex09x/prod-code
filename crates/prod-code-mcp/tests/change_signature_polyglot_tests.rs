/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::signature::{change_with, Modifiers, Param};
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

fn reference_at(file: &std::path::Path, source: &str, name: &str, occurrence: usize) -> serde_json::Value {
    let at = source
        .match_indices(name)
        .nth(occurrence)
        .expect("reference occurrence")
        .0;
    let before = &source[..at];
    let line = before.bytes().filter(|byte| *byte == b'\n').count() as u32;
    let character = before.rsplit('\n').next().unwrap_or_default().encode_utf16().count() as u32;
    serde_json::json!({
        "uri": url::Url::from_file_path(file).unwrap().to_string(),
        "range": { "start": { "line": line, "character": character } }
    })
}

async fn fake_gateway_with_references(references: Vec<serde_json::Value>) -> ScriptedGateway {
    ScriptedGateway::start(move |method, _params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => serde_json::Value::Array(references.clone()),
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_change_signature_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export function add(a: number, b: number): number {
    return a + b;
}
"#,
        ),
        (
            "main.ts",
            r#"import { add } from "./calc";

export function run() {
    return add(1, 2) + add(10, 20);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.ts");
    let main_file = root.join("main.ts");
    let main_text = fs::read_to_string(&main_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&main_file, &main_text, "add", 1),
        reference_at(&main_file, &main_text, "add", 2),
    ])
    .await;

    let params = vec![
        Param::Keep("b".to_string()),
        Param::Keep("a".to_string()),
        Param::Add {
            name: "scale".to_string(),
            ty: "number".to_string(),
            value: "1".to_string(),
        },
    ];

    let modifiers = Modifiers {
        returns: Some("number".to_string()),
        visibility: None,
        asyncness: None,
    };

    let change = change_with(
        gw.addr(),
        &root,
        &calc_file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(change.symbol, "add");
    assert!(change.applied);

    let calc_content = fs::read_to_string(&calc_file).unwrap();
    assert!(calc_content.contains("function add(b: number, a: number, scale: number = 1): number"));

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("add(2, 1, 1) + add(20, 10, 1)"));
}

#[tokio::test]
async fn test_change_signature_python_method_and_keywords() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.py",
            r#"class Calculator:
    def compute(self, x: int, y: int) -> int:
        return x + y

def caller():
    c = Calculator()
    return c.compute(10, 20) + c.compute(x=1, y=2)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.py");
    let service_text = fs::read_to_string(&service_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&service_file, &service_text, "compute", 1),
        reference_at(&service_file, &service_text, "compute", 2),
    ])
    .await;

    let params = vec![
        Param::Keep("y".to_string()),
        Param::Keep("x".to_string()),
        Param::Add {
            name: "factor".to_string(),
            ty: "int".to_string(),
            value: "2".to_string(),
        },
    ];

    let modifiers = Modifiers {
        returns: Some("int".to_string()),
        visibility: None,
        asyncness: None,
    };

    let change = change_with(
        gw.addr(),
        &root,
        &service_file,
        2,
        9,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(change.symbol, "compute");
    assert!(change.applied);

    let content = fs::read_to_string(&service_file).unwrap();
    assert!(content.contains("def compute(self, y: int, x: int, factor: int = 2) -> int:"));
    assert!(content.contains("c.compute(20, 10, 2)"));
    assert!(content.contains("c.compute(y=2, x=1, factor=2)"));
}

#[tokio::test]
async fn test_change_signature_cpp_header_and_source() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.h",
            r#"#ifndef MATH_H
#define MATH_H

int multiply(int a, int b);

#endif
"#,
        ),
        (
            "math.cpp",
            r#"#include "math.h"

int multiply(int a, int b) {
    return a * b;
}
"#,
        ),
        (
            "app.cpp",
            r#"#include "math.h"

int run() {
    return multiply(3, 4);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_cpp = root.join("math.cpp");
    let math_h = root.join("math.h");
    let app_cpp = root.join("app.cpp");
    let app_text = fs::read_to_string(&app_cpp).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&app_cpp, &app_text, "multiply", 0)])
        .await;

    let params = vec![
        Param::Keep("b".to_string()),
        Param::Keep("a".to_string()),
        Param::Add {
            name: "offset".to_string(),
            ty: "int".to_string(),
            value: "0".to_string(),
        },
    ];

    let modifiers = Modifiers {
        returns: Some("long".to_string()),
        visibility: None,
        asyncness: None,
    };

    let change = change_with(
        gw.addr(),
        &root,
        &math_cpp,
        3,
        5,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(change.symbol, "multiply");
    assert!(change.applied);

    let cpp_content = fs::read_to_string(&math_cpp).unwrap();
    assert!(cpp_content.contains("long multiply(int b, int a, int offset) {"));

    let h_content = fs::read_to_string(&math_h).unwrap();
    assert!(h_content.contains("long multiply(int b, int a, int offset);"));

    let app_content = fs::read_to_string(&app_cpp).unwrap();
    assert!(app_content.contains("multiply(4, 3, 0)"));
}

#[tokio::test]
async fn test_change_signature_refuses_to_reorder_side_effecting_arguments() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            "export function add(a: number, b: number): number {\n    return a + b;\n}\nexport function run(i: number) {\n    return add(i++, i++);\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("calc.ts");
    let before = fs::read_to_string(&file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&file, &before, "add", 1)]).await;
    let params = vec![Param::Keep("b".to_string()), Param::Keep("a".to_string())];
    let modifiers = Modifiers {
        returns: None,
        visibility: None,
        asyncness: None,
    };

    let error = change_with(
        gw.addr(),
        &root,
        &file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("argument side effects"));
    assert_eq!(fs::read_to_string(file).unwrap(), before);
}

#[tokio::test]
async fn test_change_signature_rewrites_only_analyzer_resolved_same_named_calls() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            "export function add(a: number, b: number): number { return a + b; }\n",
        ),
        (
            "main.ts",
            "import { add } from './calc';\nexport function run() { return add(1, 2); }\n",
        ),
        (
            "other.ts",
            "function add(message: string) { return message; }\nexport function other() { return add('unchanged'); }\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let declaration = root.join("calc.ts");
    let main_file = root.join("main.ts");
    let other_file = root.join("other.ts");
    let main_text = fs::read_to_string(&main_file).unwrap();
    let other_before = fs::read_to_string(&other_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&main_file, &main_text, "add", 1)]).await;
    let params = vec![Param::Keep("b".to_string()), Param::Keep("a".to_string())];

    let change = change_with(
        gw.addr(),
        &root,
        &declaration,
        1,
        17,
        &params,
        &Modifiers::default(),
        true,
        false,
    )
    .await
    .unwrap();

    assert!(change.applied);
    assert!(fs::read_to_string(main_file).unwrap().contains("add(2, 1)"));
    assert_eq!(fs::read_to_string(other_file).unwrap(), other_before);
}

#[tokio::test]
async fn test_change_signature_swift_labels() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "formatter.swift",
            r#"func format(title: String, code: Int) -> String {
    return "\(title): \(code)"
}

func test() {
    _ = format(title: "Error", code: 404)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let swift_file = root.join("formatter.swift");
    let swift_text = fs::read_to_string(&swift_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&swift_file, &swift_text, "format", 1)])
        .await;

    let params = vec![
        Param::Keep("code".to_string()),
        Param::Keep("title".to_string()),
        Param::Add {
            name: "prefix".to_string(),
            ty: "String".to_string(),
            value: "\"LOG\"".to_string(),
        },
    ];

    let modifiers = Modifiers {
        returns: Some("String".to_string()),
        visibility: None,
        asyncness: None,
    };

    let change = change_with(
        gw.addr(),
        &root,
        &swift_file,
        1,
        6,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(change.symbol, "format");
    assert!(change.applied);

    let content = fs::read_to_string(&swift_file).unwrap();
    assert!(content.contains("func format(code: Int, title: String, prefix: String = \"LOG\") -> String"));
    assert!(content.contains("format(code: 404, title: \"Error\", prefix: \"LOG\")"));
}

#[tokio::test]
async fn test_change_signature_typescript_async_modifier() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "api.ts",
            r#"export function fetchUser(id: string): string {
    return id;
}

export function load() {
    return fetchUser("123");
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let api_file = root.join("api.ts");
    let api_text = fs::read_to_string(&api_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&api_file, &api_text, "fetchUser", 1)])
        .await;

    let params = vec![Param::Keep("id".to_string())];
    let modifiers = Modifiers {
        returns: Some("Promise<string>".to_string()),
        visibility: None,
        asyncness: Some(true),
    };

    let change = change_with(
        gw.addr(),
        &root,
        &api_file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(change.symbol, "fetchUser");
    assert!(change.applied);

    let content = fs::read_to_string(&api_file).unwrap();
    assert!(content.contains("export async function fetchUser(id: string): Promise<string>"));
    assert!(content.contains("await fetchUser(\"123\")"));
}

#[tokio::test]
async fn test_change_signature_python_removes_await_when_async_disabled() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "api.py",
            "async def fetch(id):\n    return id\n\nasync def load():\n    return await fetch(\"123\")\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let api_file = root.join("api.py");
    let api_text = fs::read_to_string(&api_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&api_file, &api_text, "fetch", 1)]).await;
    let params = vec![Param::Keep("id".to_string())];
    let modifiers = Modifiers {
        returns: None,
        visibility: None,
        asyncness: Some(false),
    };

    let change = change_with(
        gw.addr(),
        &root,
        &api_file,
        1,
        11,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert!(change.applied);
    let content = fs::read_to_string(&api_file).unwrap();
    assert!(content.contains("def fetch(id):"));
    assert!(content.contains("return fetch(\"123\")"));
    assert!(!content.contains("await fetch"));
}

#[tokio::test]
async fn test_change_signature_refuses_dropped_param_used_in_body() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "worker.ts",
            r#"export function process(data: string, secret: string): string {
    return data + secret;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let worker_file = root.join("worker.ts");
    let gw = fake_gateway().await;

    let params = vec![Param::Keep("data".to_string())];
    let modifiers = Modifiers::default();

    let err = change_with(
        gw.addr(),
        &root,
        &worker_file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("parameter `secret` is still used in the function body"));
}

#[tokio::test]
async fn test_change_signature_refuses_unknown_param() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "worker.ts",
            r#"export function process(data: string): string {
    return data;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let worker_file = root.join("worker.ts");
    let gw = fake_gateway().await;

    let params = vec![
        Param::Keep("data".to_string()),
        Param::Keep("non_existent".to_string()),
    ];
    let modifiers = Modifiers::default();

    let err = change_with(
        gw.addr(),
        &root,
        &worker_file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("`non_existent` is not a declared parameter"));
}

#[tokio::test]
async fn test_change_signature_refuses_duplicate_params() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "worker.ts",
            r#"export function process(data: string): string {
    return data;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let worker_file = root.join("worker.ts");
    let gw = fake_gateway().await;

    let params = vec![
        Param::Keep("data".to_string()),
        Param::Keep("data".to_string()),
    ];
    let modifiers = Modifiers::default();

    let err = change_with(
        gw.addr(),
        &root,
        &worker_file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("duplicate parameter `data`"));
}

#[tokio::test]
async fn test_change_signature_javascript() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "greet.js",
            r#"export function greet(name, greeting) {
    return greeting + ", " + name;
}

export function main() {
    return greet("Alice", "Hello");
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let greet_file = root.join("greet.js");
    let greet_text = fs::read_to_string(&greet_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&greet_file, &greet_text, "greet", 3)])
        .await;

    let params = vec![
        Param::Keep("greeting".to_string()),
        Param::Keep("name".to_string()),
        Param::Add {
            name: "suffix".to_string(),
            ty: "".to_string(),
            value: "\"!\"".to_string(),
        },
    ];
    let modifiers = Modifiers::default();

    let change = change_with(
        gw.addr(),
        &root,
        &greet_file,
        1,
        17,
        &params,
        &modifiers,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(change.symbol, "greet");
    assert!(change.applied);

    let content = fs::read_to_string(&greet_file).unwrap();
    assert!(content.contains("function greet(greeting, name, suffix = \"!\")"));
    assert!(content.contains("greet(\"Hello\", \"Alice\", \"!\")"));
}
