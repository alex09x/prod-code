/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use prod_code_mcp::type_migration::{migrate, migrate_ext};
use serde_json::json;
use std::fs;

async fn preserves(source: &str, name: &str, was: &str, to: &str) {
    let ws = workspace(source);
    let root = ws.root();
    let file = root.join("lib.rs");
    let baseline = rustc(source);
    println!("return source={source:?}; rustc baseline={baseline:?}");
    assert!(baseline.0, "baseline must compile");
    let gw = gateway(json!([]), false, false).await;
    let plan = migrate_ext(
        gw.server.addr(),
        &root,
        &file,
        Some(name),
        None,
        None,
        to,
        false,
        false,
        false,
        false,
    )
    .await;
    println!("return preview {name}: {plan:?}");
    let plan = plan.unwrap();
    let rewritten = &plan.rewritten[0].1;
    let compiled = rustc(rewritten);
    println!(
        "return rewritten={rewritten:?}; utf8={:?}; rustc rewritten={compiled:?}",
        rewritten.as_bytes()
    );
    assert_eq!(fs::read(&file).unwrap(), source.as_bytes());
    assert_eq!(plan.was, was);
    let expected = source.replacen(&format!("-> {was}"), &format!("-> {to}"), 1);
    assert_eq!(
        plan.rewritten[0].1, expected,
        "only return-type bytes may change"
    );
    assert!(compiled.0, "rewritten source must compile");
    let (line, col) = position(source, name);
    let applied = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        to,
        false,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(applied.applied);
    assert_eq!(fs::read_to_string(&file).unwrap(), expected);
    println!("return applied {name}: {applied:?}");
    println!("return {name}: rustc before/after success; exact rewritten bytes preserved");
}

#[tokio::test]
async fn xid_continuations_on_both_sides_of_where_survive() {
    for alias in ["A\u{0301}where", "where\u{0301}"] {
        // No trailing item or semicolon can hide a return/body boundary error.
        let source = format!("type {alias} = u32; fn value() -> {alias} {{1}}");
        preserves(&source, "value", alias, "u64").await;
    }
}

#[tokio::test]
async fn trait_where_clause_and_following_items_survive() {
    preserves(
        "pub trait Factory { fn make() -> Self where Self: Sized; fn later() -> u32 { 9 } }\n",
        "make",
        "Self",
        "Option<Self>",
    )
    .await;
}

#[tokio::test]
async fn ordinary_function_where_clause_survives() {
    preserves(
        "pub fn value() -> u32 where (): Sized { 1 }\npub fn later() -> u32 { 9 }\n",
        "value",
        "u32",
        "i64",
    )
    .await;
}

#[tokio::test]
async fn original_generic_function_is_useful() {
    preserves(
        "pub fn value<T>() -> T where T: Clone { panic!() }\npub fn later() -> u32 { 9 }\n",
        "value",
        "T",
        "Option<T>",
    )
    .await;
}

#[tokio::test]
async fn generic_headers_nested_delimiters_and_macro_types_survive() {
    let cases = [
        (
            "pub fn value<F: Fn()->u32>() -> u32 where F: Sized { 1 }\n",
            "u32",
            "i64",
        ),
        (
            "pub trait Bound<const N: usize> {}\npub fn value<T: Bound<{ if 1 < 2 { 1 } else { 2 } }>>() -> T where T: Sized { panic!() }\n",
            "T",
            "Option<T>",
        ),
        (
            "pub fn value<const N: usize>() -> [u8; N] where (): Sized { panic!() }\n",
            "[u8; N]",
            "Option<[u8; N]>",
        ),
        (
            "macro_rules! ty { ({$t:ty}) => { $t }; }\npub fn value() -> ty!({u32}) where (): Sized { panic!() }\n",
            "ty!({u32})",
            "i64",
        ),
        (
            "pub fn value() -> fn() -> u32 where (): Sized { panic!() }\n",
            "fn() -> u32",
            "Option<fn() -> u32>",
        ),
        (
            "pub fn value<T /* -> ; { */: Clone>() -> Option<\nT\n> /* where ; { */ where T: Sized { None }\n",
            "Option<\nT\n>",
            "Option<Option<T>>",
        ),
        (
            "macro_rules! ty { ($t:ty) => { $t }; }\npub fn value() -> ty!{u32} { panic!() }\n",
            "ty!{u32}",
            "i64",
        ),
        (
            "pub struct Résumé;\npub fn value() -> Résumé { panic!() }\n",
            "Résumé",
            "Option<Résumé>",
        ),
        (
            "pub struct r#where;\npub fn value() -> r#where { panic!() }\n",
            "r#where",
            "Option<r#where>",
        ),
        (
            "pub trait Factory { fn value() -> u32; fn later() -> u32 { 9 } }\n",
            "u32",
            "i64",
        ),
        ("pub fn value() -> ! { panic!() }\n", "!", "()"),
    ];
    for (source, was, to) in cases {
        preserves(source, "value", was, to).await;
    }
}

#[tokio::test]
async fn no_return_never_borrows_a_later_item_or_where_bound_arrow() {
    for source in [
        "pub trait Factory { fn make(); fn later() -> u32 { 9 } }\n",
        "pub fn make<F>() where F: Fn() -> u32 {}\npub fn later() -> u32 { 9 }\n",
    ] {
        let ws = workspace(source);
        let root = ws.root();
        let file = root.join("lib.rs");
        let gw = gateway(json!([]), false, true).await;
        let (line, col) = position(source, "make");
        let result = migrate(
            gw.server.addr(),
            &root,
            &file,
            line,
            col,
            "i64",
            false,
            true,
            true,
        )
        .await;
        assert!(
            result.is_err(),
            "a later arrow is not a return type: {result:?}"
        );
        assert_eq!(fs::read(&file).unwrap(), source.as_bytes());
    }
}

#[tokio::test]
async fn clean_field_and_conversion_with_utf16_coordinates_still_work() {
    let source = "pub fn value() -> i64 { let _ = \"😀\"; let number: i32 = 1; number.into() }\n";
    let ws = workspace(source);
    let root = ws.root();
    let file = root.join("lib.rs");
    let gw = gateway(json!([]), false, false).await;
    let (line, col) = position(source, "number:");
    let result = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "i64",
        false,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(
        gw.requests
            .lock()
            .unwrap()
            .iter()
            .any(|(m, p)| m == "textDocument/references"
                && p["position"]["line"] == line - 1
                && p["position"]["character"] == col - 1)
    );
    assert!(result.applied && result.sites.is_empty());
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        source.replace("number: i32", "number: i64")
    );
    assert!(rustc(&fs::read_to_string(&file).unwrap()).0);

    let source = "pub fn value(input: i32) -> i32 { input }\n";
    ws.write("lib.rs", source);
    let (line, col) = position(source, "value");
    let converted = migrate(
        gw.server.addr(),
        &root,
        &file,
        line,
        col,
        "i64",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    assert!(converted.applied && converted.sites.is_empty());
    assert_eq!(converted.converted.len(), 1);
    assert_eq!(
        fs::read_to_string(&file).unwrap(),
        "pub fn value(input: i32) -> i64 { input.into() }\n"
    );
    assert!(rustc(&fs::read_to_string(&file).unwrap()).0);
}
