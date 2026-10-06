/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_mcp::generify::generify_polyglot;
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
async fn test_generify_typescript_basic_and_containers() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export function processItem(item: string): void {
    console.log(item);
}

export function handleArray(items: string[]): number {
    return items.length;
}

export function handlePromise(data: Promise<string>): Promise<string> {
    return data;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = fake_gateway().await;

    // 1. Basic bounded generify
    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("processItem"),
        None,
        None,
        "item",
        "Printable",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "processItem");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("export function processItem<T extends Printable>(item: T): void {"));

    // 2. Array type preservation
    let res_arr = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("handleArray"),
        None,
        None,
        "items",
        "",
        "E",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_arr.function, "handleArray");
    assert!(res_arr.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("export function handleArray<E>(items: E[]): number {"));

    // 3. Container Promise<T> preservation
    let res_prom = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("handlePromise"),
        None,
        None,
        "data",
        "",
        "P",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_prom.function, "handlePromise");
    assert!(res_prom.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("export function handlePromise<P>(data: Promise<P>): Promise<string> {"));
}

#[tokio::test]
async fn test_generify_typescript_append_to_existing_generics() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export function transform<U>(item: string, meta: U): void {
    console.log(item, meta);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = fake_gateway().await;

    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("transform"),
        None,
        None,
        "item",
        "Comparable",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "transform");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("export function transform<U, T extends Comparable>(item: T, meta: U): void {"));
}

#[tokio::test]
async fn test_generify_python_basic_and_containers() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.py",
            r#"def process(item: str) -> None:
    print(item)

def process_unbounded(item: str) -> None:
    print(item)

def process_list(items: list[str]) -> int:
    return len(items)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.py");
    let gw = fake_gateway().await;

    // 1. Python PEP 695 bounded
    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("process"),
        None,
        None,
        "item",
        "Stringable",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "process");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("def process[T: Stringable](item: T) -> None:"));

    // 2. Python PEP 695 unbounded
    let res_unbound = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("process_unbounded"),
        None,
        None,
        "item",
        "",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_unbound.function, "process_unbounded");
    assert!(res_unbound.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("def process_unbounded[T](item: T) -> None:"));

    // 3. Python container list[T]
    let res_list = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("process_list"),
        None,
        None,
        "items",
        "",
        "ItemT",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_list.function, "process_list");
    assert!(res_list.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("def process_list[ItemT](items: list[ItemT]) -> int:"));
}

#[tokio::test]
async fn test_generify_python_existing_generics() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.py",
            r#"def transform[U](item: str, meta: U) -> None:
    print(item, meta)
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.py");
    let gw = fake_gateway().await;

    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("transform"),
        None,
        None,
        "item",
        "int",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "transform");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("def transform[U, T: int](item: T, meta: U) -> None:"));
}

#[tokio::test]
async fn test_generify_cpp_definition_and_header_synchronization() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.hpp",
            r#"#ifndef SERVICE_HPP
#define SERVICE_HPP
#include <string>

void send_item(const std::string& item);

#endif
"#,
        ),
        (
            "service.cpp",
            r#"#include "service.hpp"
#include <iostream>

void send_item(const std::string& item) {
    std::cout << item << "\n";
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let cpp_file = root.join("service.cpp");
    let hpp_file = root.join("service.hpp");
    let cpp_before = fs::read_to_string(&cpp_file).unwrap();
    let hpp_before = fs::read_to_string(&hpp_file).unwrap();
    let gw = fake_gateway().await;

    let error = generify_polyglot(
        gw.addr(),
        &root,
        &cpp_file,
        Some("send_item"),
        None,
        None,
        "item",
        "Printable",
        "T",
        true,
        false,
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("cannot safely synchronize"));
    assert_eq!(fs::read_to_string(cpp_file).unwrap(), cpp_before);
    assert_eq!(fs::read_to_string(hpp_file).unwrap(), hpp_before);
}

#[tokio::test]
async fn test_generify_cpp_existing_template() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.cpp",
            r#"template<typename U>
void combine(const std::string& item, U other) {
    std::cout << item << other;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let cpp_file = root.join("service.cpp");
    let gw = fake_gateway().await;

    let res = generify_polyglot(
        gw.addr(),
        &root,
        &cpp_file,
        Some("combine"),
        None,
        None,
        "item",
        "",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "combine");
    assert!(res.applied);

    let cpp_content = fs::read_to_string(&cpp_file).unwrap();
    assert!(cpp_content.contains("template<typename U, typename T>\nvoid combine(const T& item, U other) {"));
}

#[tokio::test]
async fn test_generify_swift_labels_inout_and_optionals() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.swift",
            r#"func inspect(item: String) {
    print(item)
}

func search(for query: String?) -> Bool {
    return query != nil
}

func mutate(item: inout Int) {
    item += 1
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.swift");
    let gw = fake_gateway().await;

    // 1. Basic bounded Swift
    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("inspect"),
        None,
        None,
        "item",
        "CustomStringConvertible",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "inspect");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func inspect<T: CustomStringConvertible>(item: T) {"));

    // 2. Argument label + optional
    let res_search = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("search"),
        None,
        None,
        "query",
        "",
        "Q",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_search.function, "search");
    assert!(res_search.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func search<Q>(for query: Q?) -> Bool {"));

    // 3. inout parameter
    let res_mut = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("mutate"),
        None,
        None,
        "item",
        "Numeric",
        "N",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_mut.function, "mutate");
    assert!(res_mut.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func mutate<N: Numeric>(item: inout N) {"));
}

#[tokio::test]
async fn test_generify_go_basic_pointers_and_slices() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.go",
            r#"package service

func Output(item string) {
    println(item)
}

func OutputPointer(item *string) {
    println(*item)
}

func OutputSlice(items []string) {
    println(len(items))
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.go");
    let gw = fake_gateway().await;

    // 1. Go bounded
    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("Output"),
        None,
        None,
        "item",
        "fmt.Stringer",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "Output");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func Output[T fmt.Stringer](item T) {"));

    // 2. Go pointer
    let res_ptr = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("OutputPointer"),
        None,
        None,
        "item",
        "",
        "P",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_ptr.function, "OutputPointer");
    assert!(res_ptr.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func OutputPointer[P any](item *P) {"));

    // 3. Go slice
    let res_slice = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("OutputSlice"),
        None,
        None,
        "items",
        "any",
        "Elem",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res_slice.function, "OutputSlice");
    assert!(res_slice.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func OutputSlice[Elem any](items []Elem) {"));
}

#[tokio::test]
async fn test_generify_splits_grouped_go_parameter_before_rewriting() {
    for (parameter, expected) in [
        ("a", "func Convert[T any](a T, b string) string"),
        ("b", "func Convert[T any](a string, b T) string"),
    ] {
        let ws = Workspace::new(&[
            ("Cargo.toml", CARGO_TOML),
            (
                "service.go",
                "package service\nfunc Convert(a, b string) string { return a + b }\n",
            ),
        ]);
        let root = ws.root().to_path_buf();
        let file = root.join("service.go");
        let gw = fake_gateway().await;

        let result = generify_polyglot(
            gw.addr(),
            &root,
            &file,
            Some("Convert"),
            None,
            None,
            parameter,
            "",
            "T",
            true,
            false,
        )
        .await
        .unwrap();

        assert!(result.applied);
        assert!(fs::read_to_string(file).unwrap().contains(expected));
    }
}

#[tokio::test]
async fn test_generify_checks_discovered_caller_files_before_apply() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "api.ts",
            "export function normalize(value: string) { return value.toUpperCase(); }\n",
        ),
        (
            "main.ts",
            "import { normalize } from './api';\nexport function run() { return normalize('x'); }\n",
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("api.ts");
    let original = fs::read_to_string(&file).unwrap();
    let caller_diagnostic_requests =
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let caller_seen = std::sync::Arc::clone(&caller_diagnostic_requests);
    let remote = ScriptedGateway::start(move |method, params| {
        if method == "textDocument/diagnostic"
            && params
                .pointer("/textDocument/uri")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|uri| uri.ends_with("/main.ts"))
            && caller_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 1
        {
            serde_json::json!({
                "items": [{
                    "severity": 1,
                    "message": "caller type error",
                    "range": {
                        "start": { "line": 0, "character": 0 },
                        "end": { "line": 0, "character": 1 }
                    }
                }]
            })
        } else if method == "textDocument/diagnostic" {
            answers::no_diagnostics()
        } else {
            serde_json::Value::Null
        }
    })
    .await;

    let result = generify_polyglot(
        remote.addr(),
        &root,
        &file,
        Some("normalize"),
        None,
        None,
        "value",
        "string",
        "T",
        false,
        false,
    )
    .await
    .unwrap();

    assert_eq!(result.callers_checked, 1);
    assert!(
        result.diagnostics.iter().any(|d| d.contains("caller type error")),
        "diagnostics: {:?}; requests: {:?}",
        result.diagnostics,
        caller_diagnostic_requests.load(std::sync::atomic::Ordering::SeqCst)
    );
    assert_eq!(fs::read_to_string(file).unwrap(), original);
}

#[tokio::test]
async fn test_generify_go_existing_type_params() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.go",
            r#"package service

func Process[K comparable](item string, key K) {
    println(item)
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.go");
    let gw = fake_gateway().await;

    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("Process"),
        None,
        None,
        "item",
        "any",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "Process");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("func Process[K comparable, T any](item T, key K) {"));
}

#[tokio::test]
async fn test_generify_safety_refusals() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "service.ts",
            r#"export function calculate<T>(val: number): number {
    return val * 2;
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("service.ts");
    let gw = fake_gateway().await;

    // 1. Missing parameter
    let err_missing = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("calculate"),
        None,
        None,
        "nonexistent",
        "",
        "U",
        true,
        false,
    )
    .await;
    assert!(err_missing.is_err());
    let err_msg = err_missing.unwrap_err().to_string();
    assert!(err_msg.contains("has no parameter `nonexistent`"), "{err_msg}");

    // 2. Type param collision with existing 'T'
    let err_collision = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("calculate"),
        None,
        None,
        "val",
        "",
        "T",
        true,
        false,
    )
    .await;
    assert!(err_collision.is_err());
    let err_msg = err_collision.unwrap_err().to_string();
    assert!(err_msg.contains("already has a generic parameter `T`"), "{err_msg}");

    // 3. Invalid identifier
    let err_invalid = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("calculate"),
        None,
        None,
        "val",
        "",
        "Invalid-Type",
        true,
        false,
    )
    .await;
    assert!(err_invalid.is_err());
    let err_msg = err_invalid.unwrap_err().to_string();
    assert!(err_msg.contains("not a type parameter name"), "{err_msg}");
}

#[tokio::test]
async fn test_generify_rust_basic() {
    let ws = Workspace::new(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            r#"pub fn display_item(item: &String) {
    println!("{}", item);
}
"#,
        ),
    ]);
    let root = ws.root().to_path_buf();
    let file = root.join("src/lib.rs");
    let gw = fake_gateway().await;

    let res = generify_polyglot(
        gw.addr(),
        &root,
        &file,
        Some("display_item"),
        None,
        None,
        "item",
        "AsRef<str>",
        "T",
        true,
        false,
    )
    .await
    .unwrap();

    assert_eq!(res.function, "display_item");
    assert!(res.applied);
    let content = fs::read_to_string(&file).unwrap();
    assert!(content.contains("pub fn display_item<T: AsRef<str>>(item: &T) {"));
}
