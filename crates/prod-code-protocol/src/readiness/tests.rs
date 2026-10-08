/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::tracker::Readiness;
use super::types::{Busy, ReadySignal, needs_index, pyright_found_sources};
use serde_json::json;
use std::time::{Duration, Instant};

fn progress(token: &str, value: serde_json::Value) -> serde_json::Value {
    json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": token, "value": value } })
}

#[tokio::test]
async fn work_begun_and_not_ended_is_busy_until_it_ends() {
    let ready = Readiness::new(ReadySignal::Progress);
    assert_eq!(
        ready.busy().map(|b| b.title),
        Some("starting".to_string()),
        "a progress server is given a moment to begin"
    );
    ready.on_message(&json!({ "jsonrpc": "2.0", "id": 1, "method": "window/workDoneProgress/create", "params": { "token": "backgroundIndexProgress" } }));
    ready.on_message(&progress(
        "backgroundIndexProgress",
        json!({ "kind": "begin", "title": "indexing", "percentage": 0 }),
    ));
    ready.on_message(&progress(
        "backgroundIndexProgress",
        json!({ "kind": "report", "message": "1234/5000", "percentage": 25 }),
    ));
    let busy = ready.busy().expect("indexing");
    assert_eq!(
        (
            busy.title.as_str(),
            busy.message.as_deref(),
            busy.percentage
        ),
        ("indexing", Some("1234/5000"), Some(25))
    );
    assert!(
        busy.describe()
            .starts_with("indexing (1234/5000, 25%) for ")
    );
    ready.on_message(&progress(
        "backgroundIndexProgress",
        json!({ "kind": "end" }),
    ));
    assert_eq!(ready.busy(), None);
}

#[tokio::test]
async fn a_wait_ends_when_the_work_ends_or_when_the_time_is_up() {
    let ready = std::sync::Arc::new(Readiness::new(ReadySignal::Progress));
    ready.on_message(&progress(
        "7",
        json!({ "kind": "begin", "title": "Setting up workspace", "message": "Loading packages..." }),
    ));
    let still = ready
        .wait(Duration::from_millis(150))
        .await
        .expect("still loading");
    assert_eq!(still.message.as_deref(), Some("Loading packages..."));

    let ending = std::sync::Arc::clone(&ready);
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        ending.on_message(&progress(
            "7",
            json!({ "kind": "end", "message": "Finished loading packages." }),
        ));
    });
    let started = Instant::now();
    assert_eq!(ready.wait(Duration::from_secs(10)).await, None);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "woken by the end, not the limit"
    );
}

#[tokio::test]
async fn work_created_but_not_yet_begun_is_busy() {
    let ready = Readiness::new(ReadySignal::Progress);
    ready.on_message(&json!({ "jsonrpc": "2.0", "id": 1, "method": "window/workDoneProgress/create", "params": { "token": 9 } }));
    assert_eq!(ready.busy().map(|b| b.title), Some("starting".to_string()));
    ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 9, "value": { "kind": "begin", "title": "Setting up workspace" } } }));
    assert_eq!(
        ready.busy().map(|b| b.title),
        Some("Setting up workspace".to_string())
    );
    ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 9, "value": { "kind": "end" } } }));
    assert_eq!(ready.busy(), None);
}

#[tokio::test]
async fn a_numeric_token_and_an_end_without_a_begin_are_taken_in() {
    let ready = Readiness::new(ReadySignal::Progress);
    ready.on_message(&progress("x", json!({ "kind": "end" })));
    ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 42, "value": { "kind": "begin", "title": "Loading" } } }));
    assert_eq!(ready.busy().map(|b| b.title), Some("Loading".to_string()));
    ready.on_message(&json!({ "jsonrpc": "2.0", "method": "$/progress", "params": { "token": 42, "value": { "kind": "end" } } }));
    assert_eq!(
        ready.busy(),
        None,
        "progress was seen, so no settle window either"
    );
}

#[tokio::test]
async fn a_log_server_is_busy_until_its_line_and_others_never() {
    let pyright = Readiness::new(ReadySignal::Log(pyright_found_sources));
    assert!(pyright.known());
    assert_eq!(
        pyright.busy().and_then(|b| b.message),
        Some("setting up its project".to_string())
    );
    pyright.on_message(&json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 4, "message": "Assuming Python version 3.10.12.final.0" } }));
    assert!(pyright.busy().is_some());
    pyright.on_message(&json!({ "jsonrpc": "2.0", "method": "window/logMessage", "params": { "type": 4, "message": "Found 20000 source files" } }));
    assert_eq!(pyright.busy(), None);

    let unknown = Readiness::new(ReadySignal::Unknown);
    assert!(!unknown.known());
    assert_eq!(unknown.busy(), None);
    assert_eq!(unknown.wait(Duration::from_secs(5)).await, None);
    assert!(Readiness::new(ReadySignal::HoldsQuestions).busy().is_none());
}

#[test]
fn the_pyright_line_and_the_index_queries_are_recognised() {
    assert!(pyright_found_sources("Found 2000 source files"));
    assert!(pyright_found_sources("Found 1 source file"));
    assert!(pyright_found_sources("No source files found."));
    assert!(!pyright_found_sources("Found pyproject.toml"));
    assert!(needs_index("workspace/symbol") && needs_index("textDocument/references"));
    assert!(needs_index("textDocument/documentSymbol") && needs_index("textDocument/definition"));
    assert!(!needs_index("textDocument/hover"));
}

#[test]
fn busy_goes_over_the_wire_without_empty_members() {
    let busy = Busy {
        title: "indexing".to_string(),
        message: None,
        percentage: Some(40),
        for_ms: 3200,
    };
    let json = serde_json::to_value(&busy).unwrap();
    assert_eq!(
        json,
        json!({ "title": "indexing", "percentage": 40, "for_ms": 3200 })
    );
    assert_eq!(serde_json::from_value::<Busy>(json).unwrap(), busy);
    assert_eq!(busy.describe(), "indexing (40%) for 3 s");
}
