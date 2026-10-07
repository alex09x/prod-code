/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::wrap_return::{Wrapper, wrap_polyglot, wrap_polyglot_ext};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::fs;

const CARGO_TOML: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

async fn fake_gateway() -> ScriptedGateway {
    ScriptedGateway::start(|method, params| match method {
        "textDocument/diagnostic" => answers::no_diagnostics(),
        "textDocument/references" => {
            let uri = params["textDocument"]["uri"].as_str().unwrap_or("");
            if uri.ends_with("/src/math.rs") {
                let client_uri = uri.replace("/src/math.rs", "/src/client.rs");
                serde_json::json!([
                    {
                        "uri": client_uri,
                        "range": {
                            "start": { "line": 3, "character": 4 },
                            "end": { "line": 3, "character": 13 }
                        }
                    }
                ])
            } else if uri.ends_with("/src/main.ts") {
                serde_json::json!([
                    {
                        "uri": uri,
                        "range": {
                            "start": { "line": 6, "character": 11 },
                            "end": { "line": 6, "character": 20 }
                        }
                    }
                ])
            } else {
                serde_json::json!([])
            }
        }
        _ => serde_json::Value::Null,
    })
    .await
}

#[tokio::test]
async fn test_wrap_return_typescript_promise() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function calculate(x: number): number {
    return x * 2;
}
"#,
        ),
        (
            "client.ts",
            r#"import { calculate } from "./math";

export async function run() {
    const a = calculate(5);
    const b = calculate(10).toString();
    return a;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let client_file = root.join("client.ts");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "calculate");
    assert_eq!(res.was, "number");
    assert_eq!(res.now, "Promise<number>");
    assert_eq!(res.propagated, 2);
    assert!(res.blocked.is_empty());
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("export async function calculate(x: number): Promise<number> {"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("const a = await calculate(5);"));
    assert!(client_content.contains("const b = (await calculate(10)).toString();"));
}

#[tokio::test]
async fn wrap_return_does_not_rewrite_unrelated_alias_when_references_are_empty() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/selected.ts",
            "export function retry(): string { return \"selected\"; }\n",
        ),
        (
            "src/other.ts",
            "export function unrelated(): string { return \"other\"; }\n",
        ),
        (
            "src/caller.ts",
            "import { unrelated as retry } from \"./other\";\nexport async function run(): Promise<string> {\n  const value = retry();\n  return value;\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let selected_file = root.join("src/selected.ts");
    let caller_file = root.join("src/caller.ts");
    let selected_before = fs::read_to_string(&selected_file).unwrap();
    let caller_before = fs::read_to_string(&caller_file).unwrap();
    let gw = fake_gateway().await;

    let error = wrap_polyglot(
        gw.addr(),
        &root,
        &selected_file,
        Some("retry"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(error.to_string().contains("no references"));
    assert_eq!(fs::read_to_string(&selected_file).unwrap(), selected_before);
    assert_eq!(fs::read_to_string(&caller_file).unwrap(), caller_before);
}

#[tokio::test]
async fn wrap_return_rebases_same_file_callsite_after_signature_rewrite() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/main.ts",
            "export function calculate() {\n    const inner = () => { return 2; };\n    return [1];\n}\n\nexport async function run() {\n    return calculate()[0].toString();\n}\n",
        ),
        (
            "src/other.ts",
            "class Other { calculate(value: number) { return value; } }\nexport function other() { return new Other().calculate(5); }\n",
        ),
        (
            "src/z_other.ts",
            "class Another { calculate(value: number) { return value; } }\nexport function another() { return new Another().calculate(10); }\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("src/main.ts");
    let other_file = root.join("src/other.ts");
    let other_before = fs::read_to_string(&other_file).unwrap();
    let z_other_file = root.join("src/z_other.ts");
    let z_other_before = fs::read_to_string(&z_other_file).unwrap();
    let gateway = fake_gateway().await;
    let result = wrap_polyglot(
        gateway.addr(),
        &root,
        &file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(result.applied);
    let updated = fs::read_to_string(&file).unwrap();
    assert!(updated.contains("async function calculate()"), "{updated}");
    assert!(!updated.contains("Promise<void>"), "{updated}");
    assert!(
        updated.contains("const inner = () => { return 2; }"),
        "{updated}"
    );
    assert!(updated.contains("return [1]"), "{updated}");
    assert!(
        updated.contains("(await calculate())[0].toString()"),
        "{updated}; result={result:?}"
    );
    assert_eq!(fs::read_to_string(other_file).unwrap(), other_before);
    assert_eq!(fs::read_to_string(z_other_file).unwrap(), z_other_before);
}

#[tokio::test]
async fn test_wrap_return_typescript_blocked_caller_refusal() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function calculate(x: number): number {
    return x * 2;
}
"#,
        ),
        (
            "client.ts",
            r#"import { calculate } from "./math";

export function syncRun() {
    return calculate(5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let gw = fake_gateway().await;

    let err = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("1 call site(s) cannot propagate"));

    // With force: true, it applies
    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Promise,
        None,
        true,
        true,
    )
    .await
    .unwrap();

    assert!(res.applied);
    assert_eq!(res.blocked.len(), 1);
}

#[tokio::test]
async fn test_wrap_return_typescript_option() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "user.ts",
            r#"export function getUser(id: number): string {
    return "user_" + id;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let user_file = root.join("user.ts");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &user_file,
        Some("getUser"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "string");
    assert_eq!(res.now, "string | null");
    assert!(res.applied);

    let user_content = fs::read_to_string(&user_file).unwrap();
    assert!(user_content.contains("export function getUser(id: number): string | null {"));
}

#[tokio::test]
async fn test_wrap_return_python_option() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "db.py",
            r#"def find_user(user_id: int) -> str:
    return "User"
"#,
        ),
        (
            "service.py",
            r#"from db import find_user

def get_user_name(uid: int) -> Optional[str]:
    return find_user(uid)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let db_file = root.join("db.py");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &db_file,
        Some("find_user"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "str");
    assert_eq!(res.now, "Optional[str]");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let db_content = fs::read_to_string(&db_file).unwrap();
    assert!(db_content.contains("def find_user(user_id: int) -> Optional[str]:"));
}

#[tokio::test]
async fn test_wrap_return_python_result() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math_ops.py",
            r#"def divide(a: int, b: int) -> int:
    return a // b
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math_ops.py");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("divide"),
        None,
        None,
        Wrapper::Result,
        Some("ZeroDivisionError"),
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "int");
    assert_eq!(res.now, "Result[int, ZeroDivisionError]");
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("def divide(a: int, b: int) -> Result[int, ZeroDivisionError]:"));
    assert!(math_content.contains("return Ok(a // b)"));
}

#[tokio::test]
async fn test_wrap_return_cpp_optional() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.hpp",
            r#"int calculate(int x);
"#,
        ),
        (
            "calc.cpp",
            r#"int calculate(int x) {
    return x * 2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_cpp = root.join("calc.cpp");
    let calc_hpp = root.join("calc.hpp");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &calc_cpp,
        Some("calculate"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "int");
    assert_eq!(res.now, "std::optional<int>");
    assert!(res.applied);

    let cpp_content = fs::read_to_string(&calc_cpp).unwrap();
    assert!(cpp_content.contains("std::optional<int> calculate(int x) {"));

    let hpp_content = fs::read_to_string(&calc_hpp).unwrap();
    assert!(hpp_content.contains("std::optional<int> calculate(int x);"));
}

#[tokio::test]
async fn test_wrap_return_swift_optional_and_result() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "Fetcher.swift",
            r#"func fetch(id: Int) -> String {
    return "Item"
}
"#,
        ),
        (
            "Parser.swift",
            r#"func parse(data: String) -> Int {
    return 42
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let fetcher_file = root.join("Fetcher.swift");
    let parser_file = root.join("Parser.swift");
    let gw = fake_gateway().await;

    let res_opt = wrap_polyglot(
        gw.addr(),
        &root,
        &fetcher_file,
        Some("fetch"),
        None,
        None,
        Wrapper::Option,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_opt.now, "String?");
    assert!(res_opt.applied);
    let fetcher_content = fs::read_to_string(&fetcher_file).unwrap();
    assert!(fetcher_content.contains("func fetch(id: Int) -> String? {"));

    let res_res = wrap_polyglot(
        gw.addr(),
        &root,
        &parser_file,
        Some("parse"),
        None,
        None,
        Wrapper::Result,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_res.now, "Result<Int, Error>");
    assert!(res_res.applied);
    let parser_content = fs::read_to_string(&parser_file).unwrap();
    assert!(parser_content.contains("func parse(data: String) -> Result<Int, Error> {"));
    assert!(parser_content.contains("return .success(42)"));
}

#[tokio::test]
async fn test_wrap_return_go_result_and_pointer() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "store.go",
            r#"package main

func Load(key string) string {
    return "val"
}
"#,
        ),
        (
            "handler.go",
            r#"package main

func Handle(k string) (string, error) {
    return Load(k)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let store_file = root.join("store.go");
    let gw = fake_gateway().await;

    let res = wrap_polyglot(
        gw.addr(),
        &root,
        &store_file,
        Some("Load"),
        None,
        None,
        Wrapper::Result,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.was, "string");
    assert_eq!(res.now, "(string, error)");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let store_content = fs::read_to_string(&store_file).unwrap();
    assert!(store_content.contains("func Load(key string) (string, error) {"));
    assert!(store_content.contains("return \"val\", nil"));
}

#[tokio::test]
async fn test_wrap_return_already_wrapped_refusals() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export async function getPromise(): Promise<number> {
    return 1;
}
export function getNullable(): string | null {
    return null;
}
"#,
        ),
        (
            "db.py",
            r#"def get_opt() -> Optional[int]:
    return None
"#,
        ),
        (
            "store.go",
            r#"package main

func GetErr() (string, error) {
    return "", nil
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let db_file = root.join("db.py");
    let store_file = root.join("store.go");
    let gw = fake_gateway().await;

    let err1 = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("getPromise"),
        None,
        None,
        Wrapper::Promise,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err1.to_string().contains("already returns a `Promise`"));

    let err2 = wrap_polyglot(
        gw.addr(),
        &root,
        &math_file,
        Some("getNullable"),
        None,
        None,
        Wrapper::Option,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err2.to_string().contains("already returns an `Option`"));

    let err3 = wrap_polyglot(
        gw.addr(),
        &root,
        &db_file,
        Some("get_opt"),
        None,
        None,
        Wrapper::Option,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err3.to_string().contains("already returns an `Option`"));

    let err4 = wrap_polyglot(
        gw.addr(),
        &root,
        &store_file,
        Some("GetErr"),
        None,
        None,
        Wrapper::Result,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();
    assert!(err4.to_string().contains("already returns a `Result`"));
}

#[tokio::test]
async fn test_wrap_return_rust_custom_envelope() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/math.rs",
            r#"pub fn calculate(x: u32) -> u32 {
    let y = x * 2;
    y + 1
}
"#,
        ),
        (
            "src/client.rs",
            r#"use crate::math::calculate;

pub fn run() -> Response<u32> {
    calculate(5)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("src/math.rs");
    let client_file = root.join("src/client.rs");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "calculate");
    assert_eq!(res.was, "u32");
    assert_eq!(res.now, "Response<u32>");
    assert_eq!(res.propagated, 1);
    assert!(res.blocked.is_empty());
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("pub fn calculate(x: u32) -> Response<u32> {"));
    assert!(math_content.contains("Response::new(y + 1)"));

    let client_content = fs::read_to_string(&client_file).unwrap();
    assert!(client_content.contains("calculate(5)"));
}

#[tokio::test]
async fn test_wrap_return_rust_custom_constructor_and_explicit_return() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/service.rs",
            r#"pub fn fetch_data(id: u32) -> String {
    if id == 0 {
        return "empty".to_string();
    }
    format!("data-{id}")
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("src/service.rs");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &service_file,
        Some("fetch_data"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        Some("Response::ok"),
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "fetch_data");
    assert_eq!(res.was, "String");
    assert_eq!(res.now, "Response<String>");
    assert!(res.applied);

    let service_content = fs::read_to_string(&service_file).unwrap();
    assert!(service_content.contains("pub fn fetch_data(id: u32) -> Response<String> {"));
    assert!(service_content.contains("return Response::ok(\"empty\".to_string())"));
    assert!(service_content.contains("Response::ok(format!(\"data-{id}\"))"));
}

#[tokio::test]
async fn test_wrap_return_rust_blocked_caller_refusal() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/math.rs",
            r#"pub fn compute(x: u32) -> u32 {
    x * 2
}
"#,
        ),
        (
            "src/client.rs",
            r#"use crate::math::compute;

pub fn run() -> u32 {
    compute(5)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("src/math.rs");
    let gw = fake_gateway().await;

    let err = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &math_file,
        Some("compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("1 call site(s) cannot propagate"));

    // With force: true, the edit succeeds
    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &math_file,
        Some("compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        None,
        None,
        true,
        true,
    )
    .await
    .unwrap();
    assert!(res.applied);
    assert_eq!(res.blocked.len(), 1);
}

#[tokio::test]
async fn test_wrap_return_typescript_custom_envelope() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function calculate(x: number): number {
    return x * 2;
}
"#,
        ),
        (
            "client.ts",
            r#"import { calculate } from "./math";

export function run(): Response<number> {
    return calculate(5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "calculate");
    assert_eq!(res.was, "number");
    assert_eq!(res.now, "Response<number>");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("export function calculate(x: number): Response<number> {"));
    assert!(math_content.contains("return new Response(x * 2);"));
}

#[tokio::test]
async fn test_wrap_return_typescript_custom_constructor() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "math.ts",
            r#"export function calculate(x: number): number {
    return x * 2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let math_file = root.join("math.ts");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &math_file,
        Some("calculate"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        Some("Response.ok"),
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.now, "Response<number>");
    let math_content = fs::read_to_string(&math_file).unwrap();
    assert!(math_content.contains("return Response.ok(x * 2);"));
}

#[tokio::test]
async fn test_wrap_return_python_custom_envelope() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            r#"def compute(x: int) -> int:
    return x * 2
"#,
        ),
        (
            "client.py",
            r#"from calc import compute

def run() -> Response[int]:
    return compute(10)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.py");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &calc_file,
        Some("compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        Some("Response.create"),
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "compute");
    assert_eq!(res.was, "int");
    assert_eq!(res.now, "Response[int]");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let calc_content = fs::read_to_string(&calc_file).unwrap();
    assert!(calc_content.contains("def compute(x: int) -> Response[int]:"));
    assert!(calc_content.contains("return Response.create(x * 2)"));
}

#[tokio::test]
async fn test_wrap_return_cpp_custom_envelope() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.hpp",
            r#"int compute(int x);
"#,
        ),
        (
            "calc.cpp",
            r#"#include "calc.hpp"

int compute(int x) {
    return x * 2;
}
"#,
        ),
        (
            "client.cpp",
            r#"#include "calc.hpp"

Response<int> run() {
    return compute(5);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.cpp");
    let header_file = root.join("calc.hpp");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &calc_file,
        Some("compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        None,
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "compute");
    assert_eq!(res.was, "int");
    assert_eq!(res.now, "Response<int>");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let cpp_content = fs::read_to_string(&calc_file).unwrap();
    assert!(cpp_content.contains("Response<int> compute(int x) {"));
    assert!(cpp_content.contains("return Response<int>(x * 2);"));

    let hpp_content = fs::read_to_string(&header_file).unwrap();
    assert!(hpp_content.contains("Response<int> compute(int x);"));
}

#[tokio::test]
async fn test_wrap_return_swift_custom_envelope() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.swift",
            r#"func compute(x: Int) -> Int {
    return x * 2
}
"#,
        ),
        (
            "client.swift",
            r#"func run() -> Response<Int> {
    return compute(x: 5)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.swift");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &calc_file,
        Some("compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        Some("Response.success"),
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "compute");
    assert_eq!(res.was, "Int");
    assert_eq!(res.now, "Response<Int>");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let swift_content = fs::read_to_string(&calc_file).unwrap();
    assert!(swift_content.contains("func compute(x: Int) -> Response<Int> {"));
    assert!(swift_content.contains("return Response.success(x * 2)"));
}

#[tokio::test]
async fn test_wrap_return_go_custom_envelope() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.go",
            r#"package calc

func Compute(x int) int {
    return x * 2
}
"#,
        ),
        (
            "client.go",
            r#"package calc

func Run() Response[int] {
    return Compute(5)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.go");
    let gw = fake_gateway().await;

    let res = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &calc_file,
        Some("Compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        Some("NewResponse"),
        None,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "Compute");
    assert_eq!(res.was, "int");
    assert_eq!(res.now, "Response[int]");
    assert_eq!(res.propagated, 1);
    assert!(res.applied);

    let go_content = fs::read_to_string(&calc_file).unwrap();
    assert!(go_content.contains("func Compute(x int) Response[int] {"));
    assert!(go_content.contains("return NewResponse(x * 2)"));
}

#[tokio::test]
async fn test_wrap_return_custom_already_wrapped_refusal() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export function compute(x: number): Response<number> {
    return new Response(x * 2);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.ts");
    let gw = fake_gateway().await;

    let err = wrap_polyglot_ext(
        gw.addr(),
        &root,
        &calc_file,
        Some("compute"),
        None,
        None,
        Wrapper::Custom("Response".into()),
        None,
        None,
        false,
        false,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("already returns a `Response`"));
}
