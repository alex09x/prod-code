/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn path_scoped_unqualified_references_use_the_shared_budget() {
    let observed_budget = std::sync::Arc::new(std::sync::Mutex::new(0));
    let observed_in_call = std::sync::Arc::clone(&observed_budget);
    let symbols = vec!["common_helper".to_string()];
    let result = run_symbol_batch(
        "code_references",
        serde_json::json!({ "symbols": symbols.clone(), "path": "src/lib.rs" }),
        &symbols,
        move |args| {
            *observed_in_call.lock().unwrap() = args["budget_ms"].as_u64().unwrap();
            async { Ok(McpToolCallResult::text("path-scoped references")) }
        },
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    assert!(
        *observed_budget.lock().unwrap() > 2000,
        "a path hint should not get the default unqualified-symbol cap"
    );
}

#[tokio::test]
async fn unqualified_reference_batch_preserves_partial_results_past_its_share() {
    let symbols = vec!["common_helper".to_string()];
    let result = run_symbol_batch(
        "code_references",
        serde_json::json!({ "symbols": symbols.clone() }),
        &symbols,
        |args| async move {
            assert_eq!(args["budget_ms"].as_u64(), Some(2000));
            tokio::time::sleep(std::time::Duration::from_millis(2100)).await;
            Ok(McpToolCallResult::text(
                "common_helper matched candidates\n[Partial result: remaining candidates not queried]",
            ))
        },
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    let McpContentItem::Text { text } = &result.content[0];
    assert!(text.contains("Partial result"), "{text}");
    assert!(!text.contains("Timed out on symbol"), "{text}");
}

#[tokio::test]
async fn definition_timeout_millis_sets_the_shared_batch_budget() {
    let observed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_in_call = std::sync::Arc::clone(&observed);
    let symbols = vec!["One".to_string()];
    let result = run_symbol_batch(
        "code_definition",
        serde_json::json!({ "symbols": symbols.clone(), "timeout_millis": 200 }),
        &symbols,
        move |args| {
            observed_in_call
                .lock()
                .unwrap()
                .push(args["budget_ms"].as_u64().unwrap());
            async { Ok(McpToolCallResult::text("ok")) }
        },
    )
    .await
    .unwrap();

    assert!(!result.is_error);
    let budgets = observed.lock().unwrap();
    assert_eq!(budgets.len(), 1);
    assert!(
        (150..=200).contains(&budgets[0]),
        "timeout_millis was not used as the shared batch budget: {budgets:?}"
    );
}

/// The first lookup in a fresh checkout pays for the sync and the server's start and runs out of
/// its share, while the others answer at once: the time they left is given to it again (#2006).
#[tokio::test]
async fn a_lookup_that_ran_out_of_its_share_is_asked_again_with_what_the_others_left() {
    let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = std::sync::Arc::clone(&calls);
    let symbols: Vec<String> = ["Cold::first", "Warm::second", "Warm::third"]
        .map(String::from)
        .to_vec();
    let started = std::time::Instant::now();
    let result = run_symbol_batch(
        "code_definition",
        serde_json::json!({ "symbols": symbols.clone(), "budget_ms": 1500 }),
        &symbols,
        move |args| {
            let symbol = args["symbol"].as_str().unwrap().to_string();
            let first_time = !seen.lock().unwrap().contains(&symbol);
            seen.lock().unwrap().push(symbol.clone());
            async move {
                if symbol == "Cold::first" && first_time {
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
                Ok(McpToolCallResult::text(format!("definition of {symbol}")))
            }
        },
    )
    .await
    .unwrap();

    let McpContentItem::Text { text } = &result.content[0];
    assert!(text.contains("definition of Cold::first"), "{text}");
    assert!(!text.contains("Timed out on symbol"), "{text}");
    assert!(
        text.find("Cold::first").unwrap() < text.find("Warm::second").unwrap(),
        "the answers stay in the order asked: {text}"
    );
    assert_eq!(
        *calls.lock().unwrap(),
        ["Cold::first", "Warm::second", "Warm::third", "Cold::first"]
    );
    assert!(started.elapsed() < std::time::Duration::from_millis(1500));
}

/// A lookup that is slow both times keeps its timeout message, and the batch still ends within
/// its budget.
#[tokio::test]
async fn a_lookup_that_is_slow_both_times_stays_timed_out_within_the_budget() {
    let symbols: Vec<String> = ["Slow::one", "Fast::two"].map(String::from).to_vec();
    let started = std::time::Instant::now();
    let result = run_symbol_batch(
        "code_definition",
        serde_json::json!({ "symbols": symbols.clone(), "budget_ms": 1400 }),
        &symbols,
        |args| async move {
            if args["symbol"] == "Slow::one" {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
            Ok(McpToolCallResult::text("definition"))
        },
    )
    .await
    .unwrap();

    assert!(!result.is_error, "the fast one answered");
    let McpContentItem::Text { text } = &result.content[0];
    assert!(text.contains("Timed out on symbol 1/2"), "{text}");
    assert!(started.elapsed() < std::time::Duration::from_millis(1600));
}
