/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::gateway::{Gateway, Reply, workspace};
use prod_code_mcp::dossier::diagnose;
use std::time::{Duration, Instant};

const FIX: &str = r#"{"reason":"compiler-message","message":{"level":"error","message":"mismatched types","children":[{"message":"consider borrowing here","spans":[{"file_name":"selected/src/lib.rs","byte_start":10,"byte_end":10,"line_start":8,"suggested_replacement":"&","suggestion_applicability":"MachineApplicable"}]}]}}
"#;

#[tokio::test]
async fn delayed_test_and_fix_pass_share_one_deadline() {
    let temp = workspace();
    let gateway = Gateway::start(
        temp.path(),
        vec![
            Reply::Delayed(Duration::from_millis(1250), ""),
            Reply::Delayed(Duration::from_millis(1250), FIX),
        ],
    )
    .await;
    let started = Instant::now();
    let result = diagnose(gateway.addr, temp.path(), None, Some("chosen"), 2).await;
    let elapsed = started.elapsed();
    let calls = gateway.finish().await;
    let report = result.unwrap();
    println!(
        "budget_ms=2000 elapsed_ms={} timeouts={:?} fixes={:?}",
        elapsed.as_millis(),
        calls.iter().map(|r| r.timeout_secs).collect::<Vec<_>>(),
        report.suggested_fixes
    );
    assert_eq!(calls.len(), 2);
    assert!(
        elapsed < Duration::from_millis(2350),
        "second pass reset the deadline: {elapsed:?}"
    );
    assert_eq!(
        calls[1].timeout_secs, 1,
        "only the remaining rounded-up second belongs to the fix pass"
    );
    assert!(!report.build_errors.is_empty());
    assert!(
        report.suggested_fixes.is_empty(),
        "late suggestions must not escape the deadline"
    );
}

#[tokio::test]
async fn remaining_budget_still_allows_useful_fixes() {
    let temp = workspace();
    let gateway = Gateway::start(
        temp.path(),
        vec![
            Reply::Delayed(Duration::from_millis(1100), ""),
            Reply::Delayed(Duration::ZERO, FIX),
        ],
    )
    .await;
    let result = diagnose(gateway.addr, temp.path(), None, Some("chosen"), 3).await;
    let calls = gateway.finish().await;
    let report = result.unwrap();
    println!(
        "remaining timeouts={:?} fixes={:?}",
        calls.iter().map(|r| r.timeout_secs).collect::<Vec<_>>(),
        report.suggested_fixes
    );
    assert_eq!(calls[1].timeout_secs, 2);
    assert_eq!(
        report.suggested_fixes,
        ["selected/src/lib.rs:8: mismatched types: consider borrowing here"]
    );
}

#[tokio::test]
async fn exhausted_initial_pass_does_not_start_a_fix_pass() {
    let temp = workspace();
    let gateway = Gateway::start(
        temp.path(),
        vec![
            Reply::Delayed(Duration::from_millis(1300), ""),
            Reply::Delayed(Duration::ZERO, FIX),
        ],
    )
    .await;
    let started = Instant::now();
    let result = diagnose(gateway.addr, temp.path(), None, None, 1).await;
    let elapsed = started.elapsed();
    let calls = gateway.finish().await;
    println!(
        "initial budget_ms=1000 elapsed_ms={} requests={}",
        elapsed.as_millis(),
        calls.len()
    );
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("shared verification deadline")
    );
    assert_eq!(calls.len(), 1);
    assert!(elapsed < Duration::from_millis(1250));
}
