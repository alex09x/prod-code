/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[tokio::test]
async fn selected_outputs_do_not_hide_upstream_index_and_limit_warnings() {
    let plan = plan::Plan::parse(json!({"outputs":["body"],"steps":[
        {"id":"find","tool":"symbols"},
        {"id":"body","tool":"code_definition"}
    ]}))
    .unwrap();
    let report = runner::run(plan, |tool, _| async move {
        Ok(if tool == "symbols" {
            json!({"items":[],"truncated":true,"exhaustive":false,"indexing_notes":["indexing"]})
        } else {
            json!({"text":"body"})
        })
    })
    .await;
    assert_eq!(report["status"], "partial");
    assert!(report["steps"][0].get("value").is_none());
    assert_eq!(
        report["steps"][0]["warnings"],
        json!([
            "index_may_be_incomplete",
            "candidate_or_match_limit",
            "search_not_exhaustive"
        ])
    );
    assert_eq!(report["steps"][1]["value"]["text"], "body");
}

#[test]
fn text_only_protocol_results_remain_compatible() {
    let result = McpToolCallResult::text("plain");
    let value = serde_json::to_value(result).unwrap();
    assert!(value.get("structuredContent").is_none());
    let parsed: McpToolCallResult = serde_json::from_value(value).unwrap();
    assert!(parsed.structured_content.is_none());
}

#[test]
fn indexing_notes_have_a_structured_channel_without_losing_text_or_prior_fields() {
    let mut original = McpToolCallResult::text("body");
    original.structured_content = Some(json!({"existing":"kept"}));
    let result =
        super::super::inspection_handlers::append_indexing_notes(original, vec!["indexing".into()]);
    let structured = result.structured_content.unwrap();
    assert_eq!(structured["existing"], "kept");
    assert_eq!(structured["indexing_notes"], json!(["indexing"]));
    assert_eq!(result.content.len(), 2);
}

#[tokio::test]
async fn warning_metadata_and_memory_failures_still_obey_output_budget() {
    let steps = (0..16)
        .map(|i| json!({"id":format!("s{i:047}"),"tool":"symbols","on_error":"continue"}))
        .collect::<Vec<_>>();
    let plan = plan::Plan::parse(json!({"max_bytes":4096,"steps":steps})).unwrap();
    let report = runner::run(plan, |_, _| async {
        Ok(json!({"text":"x".repeat(270_000),"truncated":true,"exhaustive":false,"indexing_notes":["indexing"]}))
    }).await;
    assert!(serde_json::to_vec(&report).unwrap().len() <= 4096);
    for (index, step) in report["steps"].as_array().unwrap().iter().enumerate() {
        let inline = step["warnings"]
            .as_array()
            .is_some_and(|warnings| warnings.contains(&json!("index_may_be_incomplete")));
        let compacted = report["warning_steps"]["index_may_be_incomplete"]
            .as_array()
            .is_some_and(|steps| steps.contains(&json!(index)));
        assert!(inline || compacted);
    }
}

#[tokio::test]
async fn iteration_warnings_are_preserved_even_when_the_values_are_not_selected() {
    let plan = plan::Plan::parse(json!({"outputs":["last"],"steps":[
        {"id":"items","tool":"code_slice","for_each":[1,2]},
        {"id":"last","tool":"code_hover"}
    ]}))
    .unwrap();
    let report = runner::run(plan, |tool, _| async move {
        Ok(if tool == "code_slice" {
            json!({"text":"body","incomplete":true,"coverage":{"unexpanded":2}})
        } else {
            json!({"text":"hover"})
        })
    })
    .await;
    assert!(report["steps"][0].get("value").is_none());
    assert_eq!(
        report["steps"][0]["warnings"],
        json!(["operation_incomplete", "dependency_boundary"])
    );
    assert_eq!(report["status"], "partial");
}
