/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The refusals and error paths of wrapping a Rust function's return type, driven through a
//! scripted gateway standing in for rust-analyzer.

use prod_code_mcp::wrap_return::{Wrapper, wrap_rust, wrap_rust_ext};
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::Value;
use std::net::SocketAddr;

const CARGO: &str = "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
const COUNT: &str = "pub fn count(n: u32) -> u32 {\n    n\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";
const COUNT_OPTION: &str = "pub fn count(n: u32) -> Option<u32> {\n    Some(n)\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";
const COUNT_RESULT: &str = "pub fn count(n: u32) -> Result<u32, _> {\n    Ok(n)\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n";
const COUNT_RESULT_NO_HOLE: &str = "pub fn count(n: u32) -> Result<u32, E> {\n    Ok(n)\n}\n";
const OTHER: &str = "pub fn other() {}\n";
const NOTHING: &str = "pub fn nothing() {\n}\n";
const ALREADY: &str = "pub fn done() -> Option<u32> {\n    None\n}\n";

fn workspace() -> Workspace {
    Workspace::new(&[
        ("Cargo.toml", CARGO),
        (
            "src/lib.rs",
            "pub mod count;\npub mod other;\npub mod nothing;\npub mod already;\n",
        ),
        ("src/count.rs", COUNT),
        ("src/other.rs", OTHER),
        ("src/nothing.rs", NOTHING),
        ("src/already.rs", ALREADY),
    ])
}

async fn gateway(assist: Value, refs: Value) -> SocketAddr {
    ScriptedGateway::start(move |method, _params| match method {
        "prodCode/applyAssist" => assist.clone(),
        "textDocument/references" => refs.clone(),
        "textDocument/diagnostic" => answers::no_diagnostics(),
        _ => Value::Null,
    })
    .await
    .addr()
}

fn none() -> Value {
    Value::Array(Vec::new())
}

#[tokio::test]
async fn wrapping_by_symbol_adds_question_marks_at_propagating_callers() {
    let ws = workspace();
    let count = ws.path("src/count.rs");
    let remote = gateway(
        answers::whole_file(&count, COUNT, COUNT_OPTION),
        answers::locations(&count, &[(6, 10)]),
    )
    .await;
    let done = wrap_rust(
        remote,
        &ws.root(),
        &count,
        Some("count"),
        0,
        0,
        Wrapper::Option,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(
        (
            done.function.as_str(),
            done.was.as_str(),
            done.now.as_str(),
            done.propagated
        ),
        ("count", "u32", "Option<u32>", 1)
    );
    assert!(done.blocked.is_empty() && done.unmatched.is_empty() && !done.applied);
    assert_eq!(done.rewritten.len(), 1);
    assert_eq!(
        done.rewritten[0].1,
        "pub fn count(n: u32) -> Option<u32> {\n    Some(n)\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2)? * 2)\n}\n"
    );
    assert_eq!(ws.read("src/count.rs"), COUNT, "a dry run writes nothing");
}

#[tokio::test]
async fn result_wrapping_fills_in_the_error_type() {
    let ws = workspace();
    let count = ws.path("src/count.rs");
    let remote = gateway(answers::whole_file(&count, COUNT, COUNT_RESULT), none()).await;
    let done = wrap_rust(
        remote,
        &ws.root(),
        &count,
        None,
        1,
        8,
        Wrapper::Result,
        Some(" anyhow::Error "),
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(done.now, "Result<u32, anyhow::Error>");
    assert_eq!(
        done.rewritten[0].1,
        "pub fn count(n: u32) -> Result<u32, anyhow::Error> {\n    Ok(n)\n}\n\npub fn twice() -> Option<u32> {\n    Some(count(2) * 2)\n}\n"
    );
}

#[tokio::test]
async fn custom_envelope_with_a_placeholder_names_the_wrapped_type() {
    let ws = workspace();
    let count = ws.path("src/count.rs");
    let remote = gateway(Value::Null, none()).await;
    let done = wrap_rust_ext(
        remote,
        &ws.root(),
        &count,
        Some("count"),
        0,
        0,
        Wrapper::Custom("Outcome<T>".into()),
        None,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(done.now, "Outcome<u32>");
    assert!(
        done.rewritten[0]
            .1
            .starts_with("pub fn count(n: u32) -> Outcome<u32> {")
    );
}

#[tokio::test]
async fn references_the_file_does_not_confirm_are_reported() {
    let ws = workspace();
    let count = ws.path("src/count.rs");
    let remote = gateway(
        answers::whole_file(&count, COUNT, COUNT_OPTION),
        answers::locations(&count, &[(99, 1), (2, 5)]),
    )
    .await;
    let done = wrap_rust(
        remote,
        &ws.root(),
        &count,
        Some("count"),
        0,
        0,
        Wrapper::Option,
        None,
        false,
        false,
    )
    .await
    .unwrap();
    assert_eq!(done.propagated, 0);
    let mut unmatched = done.unmatched.clone();
    unmatched.sort();
    assert_eq!(
        unmatched,
        vec![
            "src/count.rs:2:5 (the analyzer places `count` here, but the file says otherwise)"
                .to_string(),
            "src/count.rs:99:1 (the position is not in the file)".to_string(),
        ]
    );
}

#[tokio::test]
async fn wrapping_refusals_name_the_reason() {
    let ws = workspace();
    let count = ws.path("src/count.rs");
    let assist_ok = || answers::whole_file(&count, COUNT, COUNT_OPTION);
    // (file, symbol, line, col, wrapper, error, assist answer, expected message)
    let cases: Vec<(
        &str,
        Option<&str>,
        u32,
        u32,
        Wrapper,
        Option<&str>,
        Value,
        &str,
    )> = vec![
        (
            "src/count.rs",
            None,
            0,
            0,
            Wrapper::Option,
            None,
            assist_ok(),
            "provide either line and character or symbol",
        ),
        (
            "src/count.rs",
            Some("missing"),
            0,
            0,
            Wrapper::Option,
            None,
            assist_ok(),
            "function `missing` not found in",
        ),
        (
            "src/count.rs",
            None,
            99,
            1,
            Wrapper::Option,
            None,
            assist_ok(),
            "the position is not inside the file",
        ),
        (
            "src/count.rs",
            None,
            1,
            1,
            Wrapper::Option,
            None,
            assist_ok(),
            "the position is not the name of a function declaration",
        ),
        (
            "src/nothing.rs",
            Some("nothing"),
            0,
            0,
            Wrapper::Option,
            None,
            assist_ok(),
            "`nothing` returns `()` implicitly; declare `-> ()` first if that is what is meant",
        ),
        (
            "src/already.rs",
            Some("done"),
            0,
            0,
            Wrapper::Option,
            None,
            assist_ok(),
            "`done` already returns a `Option`",
        ),
        (
            "src/count.rs",
            Some("count"),
            0,
            0,
            Wrapper::Promise,
            None,
            assist_ok(),
            "Rust wrap_return supports `option`, `result`, or a custom envelope type",
        ),
        (
            "src/count.rs",
            Some("count"),
            0,
            0,
            Wrapper::Result,
            None,
            assist_ok(),
            "pass `error`: the type a `Result` fails with, such as `anyhow::Error`",
        ),
        (
            "src/count.rs",
            Some("count"),
            0,
            0,
            Wrapper::Result,
            Some("  "),
            assist_ok(),
            "pass `error`",
        ),
        (
            "src/count.rs",
            Some("count"),
            0,
            0,
            Wrapper::Option,
            None,
            answers::failure("no assist here"),
            "rust-analyzer does not wrap the return type of `count` here",
        ),
        (
            "src/count.rs",
            Some("count"),
            0,
            0,
            Wrapper::Option,
            None,
            answers::whole_file(
                &ws.path("src/other.rs"),
                OTHER,
                "pub fn other() -> u8 { 1 }\n",
            ),
            "the assist did not rewrite the declaring file",
        ),
        (
            "src/count.rs",
            Some("count"),
            0,
            0,
            Wrapper::Result,
            Some("E2"),
            answers::whole_file(&count, COUNT, COUNT_RESULT_NO_HOLE),
            "the wrapped signature has no `_` error type to fill in",
        ),
    ];
    for (idx, (rel, symbol, line, col, wrapper, error, assist, message)) in
        cases.into_iter().enumerate()
    {
        let remote = gateway(assist, none()).await;
        let err = wrap_rust(
            remote,
            &ws.root(),
            &ws.path(rel),
            symbol,
            line,
            col,
            wrapper,
            error,
            false,
            false,
        )
        .await
        .expect_err("must refuse");
        let text = format!("{err:#}");
        assert!(text.contains(message), "case {idx}: {text}");
    }
    assert_eq!(ws.read("src/count.rs"), COUNT);
}
