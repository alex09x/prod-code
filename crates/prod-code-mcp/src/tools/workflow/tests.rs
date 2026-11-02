/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[test]
fn rejects_unsafe_or_invalid_plans_before_execution() {
    for input in [
        json!({"steps":[]}),
        json!({"steps":[{"id":"x","tool":"code_exec"}]}),
        json!({"steps":[{"id":"x","tool":"code_rename"}]}),
        json!({"steps":[{"id":"x","tool":"code_callers","args":{"build_index":true}}]}),
        json!({"steps":[{"id":"x","tool":"code_outline","args":{"workspace":"/other"}}]}),
        json!({"steps":[{"id":"x","tool":"code_hover","args":{"path":{"$ref":"later#/path"}}}]}),
        json!({"steps":[{"id":"x","tool":"code_hover","args":{"path":{"$ref":"$item#/path"}}}]}),
        json!({"steps":[{"id":"x","tool":"code_hover"},{"id":"x","tool":"code_hover"}]}),
        json!({"steps":[{"id":"x","tool":"code_hover","args":[] }]}),
        json!({"steps":[{"id":"x","tool":"code_hover"}],"outputs":["absent"]}),
    ] {
        assert!(plan::Plan::parse(input).is_err());
    }
}

#[tokio::test]
async fn passes_typed_addresses_and_hides_intermediates() {
    let plan = plan::Plan::parse(json!({"steps":[
        {"id":"target","tool":"resolve","args":{"symbol":"Record::load"}},
        {"id":"body","tool":"code_definition","args":{
            "path":{"$ref":"target#/path"},"line":{"$ref":"target#/line"},
            "character":{"$ref":"target#/character"}
        }}
    ]}))
    .unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = calls.clone();
    let report = runner::run(plan, move |tool, args| {
        seen.lock().unwrap().push((tool.clone(), args.clone()));
        async move {
            Ok(if tool == "resolve" {
                json!({"path":"src/lib.rs","line":12,"character":4})
            } else {
                json!({"text":"12 | fn load() {}"})
            })
        }
    })
    .await;
    assert_eq!(report["status"], "complete");
    assert_eq!(report["executed_calls"], 2);
    assert!(report["steps"][0].get("value").is_none());
    assert_eq!(report["steps"][1]["value"]["text"], "12 | fn load() {}");
    assert_eq!(
        calls.lock().unwrap()[1].1,
        json!({"path":"src/lib.rs","line":12,"character":4})
    );
}

#[tokio::test]
async fn branches_after_error_without_rerunning_completed_calls() {
    let plan = plan::Plan::parse(json!({"outputs":["lookup","fallback"],"steps":[
        {"id":"lookup","tool":"resolve","on_error":"continue"},
        {"id":"read","tool":"code_definition","when":{"step":"lookup","status":"ok"}},
        {"id":"fallback","tool":"code_outline","when":{"step":"lookup","status":"error"}}
    ]}))
    .unwrap();
    let report = runner::run(plan, |tool, _| async move {
        if tool == "resolve" {
            bail!("symbol is ambiguous; choose a file hint");
        }
        Ok(json!({"text":"candidate outline"}))
    })
    .await;
    assert_eq!(report["status"], "partial");
    assert_eq!(report["executed_calls"], 2);
    assert_eq!(report["steps"][1]["status"], "skipped");
    assert_eq!(report["steps"][2]["value"]["text"], "candidate outline");
}

#[tokio::test]
async fn expands_candidates_and_refuses_an_unbounded_fanout() {
    for count in [2, 9] {
        let plan = plan::Plan::parse(json!({"steps":[
            {"id":"find","tool":"symbols"},
            {"id":"bodies","tool":"code_definition","for_each":{"$ref":"find#/items"},"args":{"path":{"$ref":"$item#/path"}}}
        ]})).unwrap();
        let seen = Arc::new(AtomicUsize::new(0));
        let counter = seen.clone();
        let report = runner::run(plan, move |tool, args| {
            counter.fetch_add(1, Ordering::SeqCst);
            async move {
                Ok(if tool == "symbols" { json!({"items":(0..count).map(|i| json!({"path":format!("src/{i}.rs")})).collect::<Vec<_>>()}) }
                else { json!({"text":args["path"]}) })
            }
        }).await;
        if count == 2 {
            assert_eq!(report["steps"][1]["value"].as_array().unwrap().len(), 2);
            assert_eq!(seen.load(Ordering::SeqCst), 3);
        } else {
            assert_eq!(report["steps"][1]["status"], "error");
            assert_eq!(seen.load(Ordering::SeqCst), 1);
        }
    }
}

#[tokio::test]
async fn timeout_preserves_completed_steps_and_skips_remaining_operations() {
    let plan = plan::Plan::parse(json!({"timeout_ms":20,"outputs":["first"],"steps":[
        {"id":"first","tool":"code_hover"},
        {"id":"slow","tool":"code_definition"},
        {"id":"never","tool":"code_hover"}
    ]}))
    .unwrap();
    let report = runner::run(plan, |tool, _| async move {
        if tool == "code_definition" {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        Ok(json!({"text":"kept"}))
    })
    .await;
    assert_eq!(report["steps"][0]["value"]["text"], "kept");
    assert_eq!(report["steps"][1]["status"], "error");
    assert_eq!(report["steps"][2]["status"], "skipped");
    assert_eq!(report["executed_calls"], 2);
}

#[tokio::test]
async fn budget_omits_whole_values_and_keeps_valid_json() {
    let plan = plan::Plan::parse(
        json!({"max_bytes":4096,"steps":[{"id":"body","tool":"code_definition"}]}),
    )
    .unwrap();
    let report = runner::run(plan, |_, _| async {
        Ok(json!({"text":"🙂".repeat(2000)}))
    })
    .await;
    assert_eq!(report["status"], "partial");
    assert_eq!(report["truncated"], true);
    assert!(report["steps"][0].get("value").is_none());
    assert!(serde_json::to_vec(&report).unwrap().len() <= 4096);
}

#[tokio::test]
async fn workflow_path_escape_is_refused_before_network_access() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        json!({"steps":[{
            "id":"read","tool":"code_outline","args":{"path":"../secret.rs"}
        }]}),
    )
    .await
    .unwrap();
    let McpContentItem::Text { text } = &result.content[0];
    let report: Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["steps"][0]["status"], "error");
    assert!(!text.contains("Failed to connect"));
}

#[test]
fn schema_lists_one_composable_entry_point() {
    let tools = super::super::list_tools();
    let workflow = tools
        .iter()
        .find(|tool| tool.name == "code_workflow")
        .unwrap();
    assert_eq!(workflow.input_schema["properties"]["steps"]["maxItems"], 16);
    assert!(tools.iter().any(|tool| tool.name == "code_definition"));
}

#[tokio::test]
async fn a_reference_replacing_all_arguments_must_still_be_an_object() {
    let plan = plan::Plan::parse(json!({"steps":[
        {"id":"first","tool":"code_hover"},
        {"id":"second","tool":"code_definition","args":{"$ref":"first#/text"}}
    ]}))
    .unwrap();
    let report = runner::run(plan, |_, _| async {
        Ok(json!({"text":"not an argument object"}))
    })
    .await;
    assert_eq!(report["executed_calls"], 1);
    assert_eq!(report["steps"][1]["status"], "error");
}

#[test]
fn short_form_inspects_only_requested_sections_without_a_new_language() {
    let expanded = short_form::expand(
        json!({"symbol":"Cache::get","path":"src/cache.rs","include":["body","callers"]}),
    )
    .unwrap();
    let plan = plan::Plan::parse(expanded).unwrap();
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(plan.steps[0].args["symbol"], "Cache::get");
    assert_eq!(plan.steps[1].tool, "code_definition");
    assert_eq!(plan.steps[2].tool, "code_callers");
    assert_eq!(plan.outputs, ["body", "callers"]);
    assert!(short_form::expand(json!({"symbol":"Cache::get","include":["compile"]})).is_err());
    assert!(short_form::expand(json!({"symbol":"Cache::get","include":["body","body"]})).is_err());
}

#[tokio::test]
async fn many_unicode_errors_fit_the_output_budget_without_corrupting_json() {
    let steps = (0..16)
        .map(|i| json!({"id":format!("s{i:047}"),"tool":"code_hover","on_error":"continue"}))
        .collect::<Vec<_>>();
    let plan = plan::Plan::parse(json!({"max_bytes":4096,"steps":steps})).unwrap();
    let report = runner::run(plan, |_, _| async { bail!("{}", "🙂".repeat(2000)) }).await;
    assert!(serde_json::to_vec(&report).unwrap().len() <= 4096);
    assert_eq!(report["truncated"], true);
}

#[tokio::test]
async fn iteration_failure_retains_successes_without_exposing_them_as_complete() {
    let plan = plan::Plan::parse(json!({"steps":[{
        "id":"bodies","tool":"code_definition","for_each":[1,2,3],"args":{"line":{"$ref":"$item#"}}
    }]}))
    .unwrap();
    let report = runner::run(plan, |_, args| async move {
        if args["line"] == 2 {
            bail!("analyzer unavailable");
        }
        Ok(json!({"text":"first body"}))
    })
    .await;
    assert_eq!(report["status"], "partial");
    assert_eq!(report["executed_calls"], 2);
    assert_eq!(
        report["steps"][0]["partial_value"],
        json!([{"text":"first body"}])
    );
    assert_eq!(report["steps"][0]["status"], "error");
}

#[tokio::test]
async fn search_short_form_deduplicates_enclosing_hints_and_keeps_unexpanded_hits() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("sample.rs"), "fn one() {\n let a = \"needle\";\n let b = \"needle\";\n}\nconst OUTSIDE: &str = \"needle\";\n").unwrap();
    let report = search::grep(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        json!({"pattern":"needle","path":"sample.rs","limit":3}),
    )
    .await
    .unwrap();
    assert_eq!(report["items"].as_array().unwrap().len(), 1);
    assert_eq!(report["items"][0]["symbol"], "one");
    assert_eq!(report["unexpanded"].as_array().unwrap().len(), 1);
    assert_eq!(report["truncated"], true);
    let expanded = short_form::expand(json!({"grep":"needle","path":"sample.rs"})).unwrap();
    let plan = plan::Plan::parse(expanded).unwrap();
    assert_eq!(plan.steps[0].tool, "grep");
    assert_eq!(plan.steps[1].tool, "code_definition");
    assert!(plan.steps[1].for_each.is_some());
}
