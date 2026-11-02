/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use anyhow::anyhow;

#[test]
fn mutations_and_verification_require_explicit_intent_and_scope() {
    for request in [
        json!({"steps":[{"id":"write","tool":"apply","args":{"files":[]}}]}),
        json!({"steps":[{"id":"check","tool":"verify","args":{"kind":"check","path":"src/lib.rs"}}]}),
        json!({"apply":true,"steps":[{"id":"check","tool":"verify","args":{"kind":"test","path":"src"}}]}),
        json!({"apply":true,"steps":[{"id":"write","tool":"apply","on_error":"continue"}]}),
        json!({"apply":true,"steps":[{"id":"write","tool":"apply","for_each":[]}]}),
    ] {
        assert!(plan::Plan::parse(request).is_err());
    }
    assert!(
        plan::Plan::parse(json!({"steps":[{"id":"preview","tool":"preview","args":{"edits":[]}}]}))
            .is_ok()
    );
    assert!(plan::Plan::parse(json!({"apply":true,"steps":[{"id":"check","tool":"verify","args":{"kind":"test","path":"src/lib.rs","filter":"regression"}}]})).is_ok());
}

#[tokio::test]
async fn later_failure_keeps_successful_apply_effects_when_values_are_hidden() {
    let plan = plan::Plan::parse(json!({"apply":true,"outputs":["check"],"steps":[
        {"id":"write","tool":"apply"},
        {"id":"check","tool":"verify","args":{"kind":"test","path":"src/lib.rs","filter":"regression"}}
    ]})).unwrap();
    let report = runner::run(plan, |tool, _| async move {
        if tool == "apply" { return Ok(json!({"touched":["src/lib.rs"],"effects":{"operation":"apply","status":"applied","affected_files":1}})); }
        Err(operation_failure::OperationFailure {
            error:anyhow!("the regression test failed"),
            effects:json!({"operation":"verify","status":"failed"}),
        }.into())
    }).await;
    assert_eq!(report["status"], "partial");
    assert!(report["steps"][0].get("value").is_none());
    assert_eq!(report["steps"][0]["effects"]["status"], "applied");
    assert_eq!(report["steps"][1]["effects"]["status"], "failed");
}

#[tokio::test]
async fn preflight_refusal_is_not_reported_as_an_applied_or_rolled_back_edit() {
    let plan = plan::Plan::parse(json!({"apply":true,"steps":[{"id":"write","tool":"apply"},{"id":"never","tool":"code_hover"}]})).unwrap();
    let report = runner::run(plan, |_, _| async {
        Err(operation_failure::OperationFailure {
            error: anyhow!("fingerprint changed"),
            effects: json!({"operation":"apply","status":"refused","writes":0}),
        }
        .into())
    })
    .await;
    assert_eq!(report["executed_calls"], 1);
    assert_eq!(report["steps"][0]["effects"]["writes"], 0);
    assert_eq!(report["steps"][1]["status"], "skipped");
}

#[tokio::test]
async fn timeout_does_not_assert_that_an_apply_was_undone() {
    let plan = plan::Plan::parse(
        json!({"apply":true,"timeout_ms":10,"steps":[{"id":"write","tool":"apply"}]}),
    )
    .unwrap();
    let report = runner::run(plan, |_, _| async {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        Ok(json!({"effects":{"operation":"apply","status":"applied"}}))
    })
    .await;
    assert_eq!(report["status"], "partial");
    assert_eq!(report["steps"][0]["effects"]["status"], "not_confirmed");
}
