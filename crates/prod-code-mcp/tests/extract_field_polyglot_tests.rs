/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::extract_field::extract_polyglot;
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
async fn extract_field_rejects_compile_verification_for_non_rust() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/main.ts", "export function f() { return 42; }\n"),
    ]);
    let root = ws.root();
    let remote = fake_gateway().await;
    let err = prod_code_mcp::tools::execute_tool(
        remote.addr(),
        &root,
        "code_extract_field",
        serde_json::json!({
            "path": "src/main.ts",
            "expression": "42",
            "name": "answer",
            "apply": true,
            "verify": "compile"
        }),
    )
    .await
    .expect_err("unsupported compile verification must be rejected");
    assert!(format!("{err:#}").contains("only supported for Rust"));
    assert_eq!(ws.read("src/main.ts"), "export function f() { return 42; }\n");
}

#[tokio::test]
async fn test_extract_field_typescript() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export class Calculator {
    compute(factor: number): number {
        return 100 * factor;
    }
}
"#,
        ),
        (
            "client.ts",
            r#"import { Calculator } from "./calc";

export function run() {
    const c = new Calculator();
    return c.compute(2);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.ts");
    let gw = fake_gateway().await;

    // Selection on `100` at line 3, column 16 to line 3, column 19
    let res = extract_polyglot(
        gw.addr(),
        &root,
        &calc_file,
        (3, 16),
        (3, 19),
        "baseVal",
        Some("number"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Calculator");
    assert_eq!(res.method, "compute");
    assert_eq!(res.name, "baseVal");
    assert_eq!(res.init, "100");
    assert_eq!(res.replaced, 1);
    assert_eq!(res.constructors, 1);
    assert!(res.applied);

    let content = fs::read_to_string(&calc_file).unwrap();
    assert!(content.contains("baseVal: number = 100;"), "{content}");
    assert!(content.contains("return this.baseVal * factor;"), "{content}");
}

#[tokio::test]
async fn test_extract_field_in_static_method_creates_static_field() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "box.ts",
            "export class Box {\n    static getValue(): number {\n        return 42;\n    }\n}\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let box_file = root.join("box.ts");
    let gw = fake_gateway().await;
    let result = extract_polyglot(
        gw.addr(),
        &root,
        &box_file,
        (3, 16),
        (3, 18),
        "MAGIC",
        Some("number"),
        None,
        false,
        false,
        false,
    )
    .await
    .unwrap();
    let rewritten = &result.rewritten[0].1;
    assert!(rewritten.contains("static MAGIC: number = 42;"), "{rewritten}");
    assert!(rewritten.contains("return this.MAGIC;"), "{rewritten}");
}

#[tokio::test]
async fn test_extract_field_python_with_init() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "worker.py",
            r#"class Worker:
    def __init__(self, name: str):
        self.name = name

    def work(self) -> str:
        return "heavy task"
"#,
        ),
        (
            "main.py",
            r#"from worker import Worker

w = Worker("Alice")
print(w.work())
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let worker_file = root.join("worker.py");
    let gw = fake_gateway().await;

    // Selection on `"heavy task"` at line 6, col 16 to line 6, col 28
    let res = extract_polyglot(
        gw.addr(),
        &root,
        &worker_file,
        (6, 16),
        (6, 28),
        "task",
        Some("str"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Worker");
    assert_eq!(res.method, "work");
    assert_eq!(res.name, "task");
    assert_eq!(res.replaced, 1);
    assert_eq!(res.constructors, 1);
    assert!(res.applied);

    let content = fs::read_to_string(&worker_file).unwrap();
    assert!(content.contains("self.task = \"heavy task\""), "{content}");
    assert!(content.contains("self.task = \"heavy task\"\n    def work("), "{content}");
    assert!(content.contains("return self.task"), "{content}");
}

#[tokio::test]
async fn test_extract_field_python_without_init() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "store.py",
            r#"class Store:
    def limit(self) -> int:
        return 1024
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let store_file = root.join("store.py");
    let gw = fake_gateway().await;

    // Selection on `1024` at line 3, col 16 to line 3, col 20
    let res = extract_polyglot(
        gw.addr(),
        &root,
        &store_file,
        (3, 16),
        (3, 20),
        "cap",
        Some("int"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Store");
    assert_eq!(res.method, "limit");
    assert_eq!(res.name, "cap");
    assert!(res.applied);

    let content = fs::read_to_string(&store_file).unwrap();
    assert!(content.contains("cap: int = 1024"), "{content}");
    assert!(content.contains("return self.cap"), "{content}");
}

#[tokio::test]
async fn test_extract_field_refuses_same_named_go_literal_in_another_package() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "store.go",
            r#"package store

type Store struct {
    entries []int
}

func (s *Store) Limit() int {
    return 1024
}
"#,
        ),
        (
            "main.go",
            r#"package main

type Store struct {
    entries []int
}

func makeStore() Store {
    return Store{
        entries: nil,
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let store_file = root.join("store.go");
    let main_file = root.join("main.go");
    let store_before = fs::read_to_string(&store_file).unwrap();
    let main_before = fs::read_to_string(&main_file).unwrap();
    let gw = fake_gateway().await;

    // Selection on `1024` in store.go at line 8, col 12 to line 8, col 16
    let err = extract_polyglot(
        gw.addr(),
        &root,
        &store_file,
        (8, 12),
        (8, 16),
        "Cap",
        Some("int"),
        None,
        false,
        true,
        false,
    )
    .await
    .expect_err("an unrelated package's same-named Go literal must not be modified");

    assert!(format!("{err:#}").contains("another package"), "{err:#}");
    assert_eq!(fs::read_to_string(&store_file).unwrap(), store_before);
    assert_eq!(fs::read_to_string(&main_file).unwrap(), main_before);
}

#[tokio::test]
async fn test_extract_field_cpp() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "server.cpp",
            r#"class Server {
public:
    int timeout() {
        return 30;
    }
};
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let server_file = root.join("server.cpp");
    let gw = fake_gateway().await;

    // Selection on `30` at line 4, col 16 to line 4, col 18
    let res = extract_polyglot(
        gw.addr(),
        &root,
        &server_file,
        (4, 16),
        (4, 18),
        "timeout_ms",
        Some("int"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Server");
    assert_eq!(res.method, "timeout");
    assert_eq!(res.name, "timeout_ms");
    assert!(res.applied);

    let content = fs::read_to_string(&server_file).unwrap();
    assert!(content.contains("int timeout_ms = 30;"), "{content}");
    assert!(content.contains("return this->timeout_ms;"), "{content}");
}

#[tokio::test]
async fn test_extract_field_swift() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "engine.swift",
            r#"class Engine {
    func power() -> Int {
        return 500
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let engine_file = root.join("engine.swift");
    let gw = fake_gateway().await;

    // Selection on `500` at line 3, col 16 to line 3, col 19
    let res = extract_polyglot(
        gw.addr(),
        &root,
        &engine_file,
        (3, 16),
        (3, 19),
        "maxPower",
        Some("Int"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.owner, "Engine");
    assert_eq!(res.method, "power");
    assert_eq!(res.name, "maxPower");
    assert!(res.applied);

    let content = fs::read_to_string(&engine_file).unwrap();
    assert!(content.contains("var maxPower: Int = 500"), "{content}");
    assert!(content.contains("return self.maxPower"), "{content}");
}

#[tokio::test]
async fn test_extract_field_refuse_parameter_mention() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"class Calc {
    add(x: number): number {
        return x + 10;
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.ts");
    let gw = fake_gateway().await;

    // Selection on `x + 10` at line 3, col 16 to line 3, col 22
    let err = extract_polyglot(
        gw.addr(),
        &root,
        &calc_file,
        (3, 16),
        (3, 22),
        "total",
        Some("number"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("mentions parameter `x`"), "unexpected error: {msg}");
}

#[tokio::test]
async fn test_extract_field_refuse_local_mention() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            r#"class Calc:
    def compute(self) -> int:
        factor = 2
        return factor * 5
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.py");
    let gw = fake_gateway().await;

    // Selection on `factor * 5` at line 4, col 16 to line 4, col 26
    let err = extract_polyglot(
        gw.addr(),
        &root,
        &calc_file,
        (4, 16),
        (4, 26),
        "total",
        Some("int"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("mentions local `factor`"), "unexpected error: {msg}");
}

#[tokio::test]
async fn test_extract_field_refuse_collision() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"class Calc {
    cap: number = 0;
    limit(): number {
        return 100;
    }
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.ts");
    let gw = fake_gateway().await;

    // Selection on `100` at line 4, col 16 to line 4, col 19
    let err = extract_polyglot(
        gw.addr(),
        &root,
        &calc_file,
        (4, 16),
        (4, 19),
        "cap",
        Some("number"),
        None,
        false,
        true,
        false,
    )
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("already has a field `cap`"), "unexpected error: {msg}");
}

#[tokio::test]
async fn test_extract_field_replace_all() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.py",
            r#"class Calc:
    def compute(self) -> int:
        a = 42
        b = 42
        message = "42"
        # 42
        value42 = 0
        return a + b
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.py");
    let gw = fake_gateway().await;

    // Selection on first `42` at line 3, col 13 to line 3, col 15
    let res = extract_polyglot(
        gw.addr(),
        &root,
        &calc_file,
        (3, 13),
        (3, 15),
        "magic",
        Some("int"),
        None,
        true, // replace_all
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.replaced, 2);
    assert!(res.applied);

    let content = fs::read_to_string(&calc_file).unwrap();
    assert!(content.contains("a = self.magic"), "{content}");
    assert!(content.contains("b = self.magic"), "{content}");
    assert!(content.contains("message = \"42\""), "{content}");
    assert!(content.contains("# 42"), "{content}");
    assert!(content.contains("value42 = 0"), "{content}");
}
