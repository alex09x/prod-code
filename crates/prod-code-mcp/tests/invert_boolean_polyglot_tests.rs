/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::invert_boolean::invert_polyglot;
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
async fn test_invert_boolean_typescript_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function isValid(x: number): boolean {
    return x > 0;
}
"#,
        ),
        (
            "client.ts",
            r#"import { isValid } from "./math";

export function check() {
    const a = isValid(5);
    const b = !isValid(10);
    const c = isValid(15).toString();
    return a && b && c;
}
"#,
        ),
        (
            "other.ts",
            "export class External { isValid(x: number) { return x > 0; } }\nexport function unrelated(external: External) { return external.isValid(5); }\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let other_file = root.join("other.ts");
    let other_before = fs::read_to_string(&other_file).unwrap();
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "isValid", 1),
        reference_at(&client_file, &client_text, "isValid", 2),
        reference_at(&client_file, &client_text, "isValid", 3),
    ])
    .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "isValid");
    assert_eq!(res.now, "isInvalid");
    assert_eq!(res.negated, 2);
    assert_eq!(res.cancelled, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("export function isInvalid(x: number): boolean {"));
    assert!(math_content.contains("return !(x > 0);"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("import { isInvalid } from \"./math\";"));
    assert!(client_content.contains("!isInvalid(5)"));
    assert!(client_content.contains("isInvalid(10)"));
    assert!(client_content.contains("(!isInvalid(15)).toString()"));
    assert_eq!(fs::read_to_string(other_file).unwrap(), other_before);
}

#[tokio::test]
async fn test_invert_boolean_refuses_unresolved_import_alias_calls() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            "export function isValid(x: number): boolean { return x > 0; }\n",
        ),
        (
            "client.ts",
            "import { isValid as check } from './math';\nexport function run() {\n    return check(5);\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let math_before = fs::read_to_string(&math_file).unwrap();
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&client_file, &client_text, "check", 0)])
        .await;

    let error = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("alias"));
    assert_eq!(fs::read_to_string(math_file).unwrap(), math_before);
    assert_eq!(fs::read_to_string(client_file).unwrap(), client_text);
}

#[tokio::test]
async fn test_invert_boolean_refuses_to_write_when_analyzer_returns_no_references() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            "export function isValid(x: number): boolean { return x > 0; }\n",
        ),
        (
            "client.ts",
            "import { isValid } from './math';\nexport function run() { return isValid(5); }\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let math_before = fs::read_to_string(&math_file).unwrap();
    let client_before = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway().await;

    let error = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("references for `isValid` were empty"));
    assert_eq!(fs::read_to_string(math_file).unwrap(), math_before);
    assert_eq!(fs::read_to_string(client_file).unwrap(), client_before);
}

#[tokio::test]
async fn test_invert_boolean_python_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math_mod.py",
            r#"def is_valid(x: int) -> bool:
    """Check validity."""
    if x == 0:
        return False
    return x > 0
"#,
        ),
        (
            "client.py",
            r#"from math_mod import is_valid

def run():
    a = is_valid(5)
    b = not is_valid(10)
    return a and b
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math_mod.py");
    let client_file = root.join("client.py");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![
        reference_at(&client_file, &client_text, "is_valid", 1),
        reference_at(&client_file, &client_text, "is_valid", 2),
    ])
    .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("is_valid"),
        "is_invalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "is_valid");
    assert_eq!(res.now, "is_invalid");
    assert_eq!(res.negated, 1);
    assert_eq!(res.cancelled, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("def is_invalid(x: int) -> bool:"));
    assert!(math_content.contains("\"\"\"Check validity.\"\"\""));
    assert!(math_content.contains("return True"));
    assert!(math_content.contains("return not (x > 0)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("from math_mod import is_invalid"));
    assert!(client_content.contains("not is_invalid(5)"));
    assert!(client_content.contains("b = is_invalid(10)"));
}

#[tokio::test]
async fn test_invert_boolean_cpp_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.cpp",
            r#"bool isValid(int x) {
    return x > 0;
}
"#,
        ),
        (
            "client.cpp",
            r#"bool isValid(int x);

int run() {
    return isValid(10) ? 1 : 0;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.cpp");
    let client_file = root.join("client.cpp");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&client_file, &client_text, "isValid", 1)])
        .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "isValid");
    assert_eq!(res.now, "isInvalid");
    assert_eq!(res.negated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("bool isInvalid(int x) {"));
    assert!(math_content.contains("return !(x > 0);"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("bool isInvalid(int x);"), "{client_content}");
    assert!(client_content.contains("!isInvalid(10)"));
}

#[tokio::test]
async fn test_invert_boolean_swift_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.swift",
            r#"func isValid(x: Int) -> Bool {
    return x > 0
}
"#,
        ),
        (
            "client.swift",
            r#"func test() -> Bool {
    return isValid(x: 10)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.swift");
    let client_file = root.join("client.swift");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&client_file, &client_text, "isValid", 0)])
        .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "isValid");
    assert_eq!(res.now, "isInvalid");
    assert_eq!(res.negated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("func isInvalid(x: Int) -> Bool {"));
    assert!(math_content.contains("return !(x > 0)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("!isInvalid(x: 10)"));
}

#[tokio::test]
async fn test_invert_boolean_go_multi_file() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.go",
            r#"package math

func IsValid(x int) bool {
    return x > 0
}
"#,
        ),
        (
            "client.go",
            r#"package main

import "math"

func run() bool {
    return math.IsValid(10)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.go");
    let client_file = root.join("client.go");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&client_file, &client_text, "IsValid", 0)])
        .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("IsValid"),
        "IsInvalid",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "IsValid");
    assert_eq!(res.now, "IsInvalid");
    assert_eq!(res.negated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("func IsInvalid(x int) bool {"));
    assert!(math_content.contains("return !(x > 0)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("!math.IsInvalid(10)"));
}

#[tokio::test]
async fn test_invert_boolean_recursive_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function isValid(n: number): boolean {
    if (n <= 0) return true;
    return isValid(n - 1);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let math_text = fs::read_to_string(&math_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&math_file, &math_text, "isValid", 1)])
        .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await;

    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("calls itself; invert a recursive predicate by hand"),
        "{err}"
    );
}

#[tokio::test]
async fn test_invert_boolean_function_used_as_value_refused() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function isValid(n: number): boolean {
    return n > 0;
}
"#,
        ),
        (
            "client.ts",
            r#"import { isValid } from "./math";

export function check() {
    const fnRef = isValid;
    return fnRef(5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let client_text = fs::read_to_string(&client_file).unwrap();
    let gw = fake_gateway_with_references(vec![reference_at(&client_file, &client_text, "isValid", 1)])
        .await;

    let res = invert_polyglot(
        gw.addr(),
        &root,
        &math_file,
        None,
        None,
        Some("isValid"),
        "isInvalid",
        true,
        false,
    )
    .await;

    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.contains("used as a value"), "{err}");
}
