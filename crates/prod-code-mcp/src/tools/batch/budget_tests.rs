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
