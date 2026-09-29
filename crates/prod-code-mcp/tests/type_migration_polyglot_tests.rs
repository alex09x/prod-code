use prod_code_mcp::type_migration::migrate_ext;
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
async fn test_migrate_type_typescript_transitive() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "calc.ts",
            r#"export function compute(val: number): number {
    const doubled: number = val * 2;
    return doubled;
}
"#,
        ),
        (
            "main.ts",
            r#"import { compute } from "./calc";

export function run() {
    const result: number = compute(10);
    return result;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let calc_file = root.join("calc.ts");
    let main_file = root.join("main.ts");
    let gw = fake_gateway().await;

    let res = migrate_ext(
        gw.addr(),
        &root,
        &calc_file,
        Some("val"),
        None,
        None,
        "string",
        false,
        true, // transitive
        true, // apply
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "val");
    assert_eq!(res.was, "number");
    assert_eq!(res.now, "string");
    assert!(res.transitive_count >= 3, "expected >= 3 transitive migrations, got {}", res.transitive_count);
    assert!(res.applied);

    let calc_content = fs::read_to_string(&calc_file).unwrap();
    assert!(calc_content.contains("function compute(val: string): string"), "{calc_content}");
    assert!(calc_content.contains("const doubled: string = val * 2;"), "{calc_content}");

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("const result: string = compute(10);"), "{main_content}");
}

#[tokio::test]
async fn test_migrate_type_python_transitive() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.py",
            r#"def calculate_fee(balance: int) -> int:
    fee: int = balance * 2
    return fee
"#,
        ),
        (
            "app.py",
            r#"from service import calculate_fee

def run():
    total: int = calculate_fee(100)
    return total
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.py");
    let app_file = root.join("app.py");
    let gw = fake_gateway().await;

    let res = migrate_ext(
        gw.addr(),
        &root,
        &service_file,
        Some("balance"),
        None,
        None,
        "float",
        false,
        true, // transitive
        true, // apply
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "balance");
    assert_eq!(res.was, "int");
    assert_eq!(res.now, "float");
    assert!(res.transitive_count >= 3, "expected >= 3 transitive migrations, got {}", res.transitive_count);
    assert!(res.applied);

    let service_content = fs::read_to_string(&service_file).unwrap();
    assert!(service_content.contains("def calculate_fee(balance: float) -> float:"), "{service_content}");
    assert!(service_content.contains("fee: float = balance * 2"), "{service_content}");

    let app_content = fs::read_to_string(&app_file).unwrap();
    assert!(app_content.contains("total: float = calculate_fee(100)"), "{app_content}");
}

#[tokio::test]
async fn test_migrate_type_go_transitive() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.go",
            r#"package main

func Compute(count int32) int32 {
    var doubled int32 = count * 2
    return doubled
}
"#,
        ),
        (
            "caller.go",
            r#"package main

func Execute() {
    var res int32 = Compute(10)
    _ = res
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let service_file = root.join("service.go");
    let caller_file = root.join("caller.go");
    let gw = fake_gateway().await;

    let res = migrate_ext(
        gw.addr(),
        &root,
        &service_file,
        Some("count"),
        None,
        None,
        "int64",
        false,
        true, // transitive
        true, // apply
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "count");
    assert_eq!(res.was, "int32");
    assert_eq!(res.now, "int64");
    assert!(res.transitive_count >= 3, "expected >= 3 transitive migrations, got {}", res.transitive_count);
    assert!(res.applied);

    let service_content = fs::read_to_string(&service_file).unwrap();
    assert!(service_content.contains("func Compute(count int64) int64 {"), "{service_content}");
    assert!(service_content.contains("var doubled int64 = count * 2"), "{service_content}");

    let caller_content = fs::read_to_string(&caller_file).unwrap();
    assert!(caller_content.contains("var res int64 = Compute(10)"), "{caller_content}");
}

#[tokio::test]
async fn test_migrate_type_cpp_transitive_and_header_sync() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "engine.h",
            r#"#ifndef ENGINE_H
#define ENGINE_H

int calculate(int count);

#endif
"#,
        ),
        (
            "engine.cpp",
            r#"#include "engine.h"

int calculate(int count) {
    int result = count * 2;
    return result;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let header_file = root.join("engine.h");
    let cpp_file = root.join("engine.cpp");
    let gw = fake_gateway().await;

    let res = migrate_ext(
        gw.addr(),
        &root,
        &cpp_file,
        Some("count"),
        None,
        None,
        "int64_t",
        false,
        true, // transitive
        true, // apply
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "count");
    assert_eq!(res.was, "int");
    assert_eq!(res.now, "int64_t");
    assert!(res.applied);

    let cpp_content = fs::read_to_string(&cpp_file).unwrap();
    assert!(cpp_content.contains("int64_t calculate(int64_t count)"), "{cpp_content}");
    assert!(cpp_content.contains("int64_t result = count * 2;"), "{cpp_content}");

    let header_content = fs::read_to_string(&header_file).unwrap();
    assert!(header_content.contains("int64_t calculate(int64_t count);"), "{header_content}");
}

#[tokio::test]
async fn test_migrate_type_swift_transitive() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "account.swift",
            r#"func getBalance(amount: Int) -> Int {
    let updated: Int = amount + 50
    return updated
}
"#,
        ),
        (
            "main.swift",
            r#"func run() {
    let finalBalance: Int = getBalance(amount: 100)
    print(finalBalance)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let account_file = root.join("account.swift");
    let main_file = root.join("main.swift");
    let gw = fake_gateway().await;

    let res = migrate_ext(
        gw.addr(),
        &root,
        &account_file,
        Some("amount"),
        None,
        None,
        "Double",
        false,
        true, // transitive
        true, // apply
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "amount");
    assert_eq!(res.was, "Int");
    assert_eq!(res.now, "Double");
    assert!(res.transitive_count >= 3, "expected >= 3 transitive migrations, got {}", res.transitive_count);
    assert!(res.applied);

    let account_content = fs::read_to_string(&account_file).unwrap();
    assert!(account_content.contains("func getBalance(amount: Double) -> Double {"), "{account_content}");
    assert!(account_content.contains("let updated: Double = amount + 50"), "{account_content}");

    let main_content = fs::read_to_string(&main_file).unwrap();
    assert!(main_content.contains("let finalBalance: Double = getBalance(amount: 100)"), "{main_content}");
}

#[tokio::test]
async fn test_migrate_type_symbol_addressing_without_line_col() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "config.ts",
            r#"export interface Config {
    timeout: number;
    retries: number;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let config_file = root.join("config.ts");
    let gw = fake_gateway().await;

    let res = migrate_ext(
        gw.addr(),
        &root,
        &config_file,
        Some("timeout"),
        None,
        None,
        "string",
        false,
        false, // transitive
        true,  // apply
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.symbol, "timeout");
    assert_eq!(res.was, "number");
    assert_eq!(res.now, "string");
    assert!(res.applied);

    let content = fs::read_to_string(&config_file).unwrap();
    assert!(content.contains("timeout: string;"), "{content}");
    assert!(content.contains("retries: number;"), "{content}");
}

#[tokio::test]
async fn test_migrate_type_refusal_and_force() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            r#"pub struct Limiter {
    pub timeout_secs: u64,
}

pub fn use_it(r: &Limiter) -> u64 {
    r.timeout_secs
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let lib_file = root.join("src/lib.rs");

    let pulls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let gw = ScriptedGateway::start(move |method, _params| match method {
        "textDocument/references" => serde_json::json!([]),
        "textDocument/diagnostic"
            if pulls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                .is_multiple_of(2) =>
        {
            serde_json::json!({ "kind": "full", "items": [] })
        }
        "textDocument/diagnostic" => serde_json::json!({
            "kind": "full",
            "items": [
                {
                    "severity": 1,
                    "code": "E0308",
                    "message": "expected u64, found Duration",
                    "range": { "start": { "line": 5, "character": 4 }, "end": { "line": 5, "character": 18 } }
                }
            ]
        }),
        _ => serde_json::Value::Null,
    })
    .await;

    // Without force, apply: true must fail
    let err = migrate_ext(
        gw.addr(),
        &root,
        &lib_file,
        Some("timeout_secs"),
        None,
        None,
        "std::time::Duration",
        false,
        false,
        true,  // apply
        false, // force = false
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("1 site(s) do not fit"), "{err:#}");

    // With force: true, it succeeds and writes
    let forced = migrate_ext(
        gw.addr(),
        &root,
        &lib_file,
        Some("timeout_secs"),
        None,
        None,
        "std::time::Duration",
        false,
        false,
        true, // apply
        true, // force = true
    )
    .await
    .unwrap();

    assert!(forced.applied);
    let content = fs::read_to_string(&lib_file).unwrap();
    assert!(content.contains("pub timeout_secs: std::time::Duration,"), "{content}");
}
