/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! #2089: a run longer than the detach window answers with its ID and keeps going; its result
//! is returned by `code_run_result`, once.

use super::*;

fn text(value: &Value) -> String {
    value
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn run_id(answer: &Value) -> String {
    let text = text(answer);
    let start = text.find("`run-").expect("a run ID") + 1;
    let end = start + text[start..].find('`').unwrap();
    text[start..end].to_string()
}

#[test]
fn only_command_tools_are_detached_and_zero_turns_it_off() {
    assert_eq!(
        detach_after("code_test", &json!({})),
        Some(DEFAULT_DETACH_AFTER)
    );
    assert_eq!(
        detach_after("code_exec", &json!({"detach_after_secs": 30})),
        Some(Duration::from_secs(30))
    );
    assert_eq!(
        detach_after("code_exec", &json!({"detach_after_secs": 0})),
        None
    );
    assert_eq!(detach_after("code_definition", &json!({})), None);
}

#[tokio::test]
async fn a_run_that_finishes_in_time_is_answered_as_it_was() {
    let result = json!({"content": [{"type": "text", "text": "ok"}], "isError": false});
    let expected = result.clone();
    let answer = run_detachable("code_exec", Duration::from_secs(5), async move { result }).await;
    assert_eq!(answer, expected);
}

#[tokio::test]
async fn a_long_run_answers_with_its_id_and_its_result_is_returned_once() {
    let (release, released) = tokio::sync::oneshot::channel::<()>();
    let answer = run_detachable("code_test", Duration::from_millis(50), async move {
        let _ = released.await;
        json!({"content": [{"type": "text", "text": "5 passed"}], "isError": false})
    })
    .await;
    assert_eq!(answer["isError"], json!(false), "{answer}");
    assert!(text(&answer).contains("code_run_result"), "{answer}");
    let id = run_id(&answer);

    let pending = run_result(&json!({"run": id, "wait_secs": 0})).await;
    assert!(text(&pending).contains("still running"), "{pending}");

    release.send(()).unwrap();
    let done = run_result(&json!({"run": id, "wait_secs": 5})).await;
    assert_eq!(text(&done), "5 passed", "{done}");

    let again = run_result(&json!({"run": id})).await;
    assert_eq!(again["isError"], json!(true), "{again}");
    assert!(text(&again).contains("no run"), "{again}");
}

#[tokio::test]
async fn a_result_call_without_a_run_is_an_error() {
    let answer = run_result(&json!({})).await;
    assert_eq!(answer["isError"], json!(true));
}
