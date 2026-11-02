/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;

#[test]
fn rejects_hidden_symbol_batches_but_accepts_scalar_references() {
    for args in [json!({"symbols":["A","B"]}), json!({"symbol":["A","B"]})] {
        assert!(
            plan::Plan::parse(
                json!({"steps":[{"id":"body","tool":"code_definition","args":args}]})
            )
            .is_err()
        );
    }
    assert!(
        plan::Plan::parse(json!({"steps":[
            {"id":"find","tool":"symbols","args":{"query":"one"}},
            {"id":"body","tool":"code_definition","args":{"symbol":{"$ref":"find#/name"}}}
        ]}))
        .is_ok()
    );
}

#[tokio::test]
async fn resolved_batch_arguments_are_rejected_before_another_operation() {
    let plan = plan::Plan::parse(json!({"steps":[
        {"id":"first","tool":"code_hover"},
        {"id":"second","tool":"code_definition","args":{"$ref":"first#"}}
    ]}))
    .unwrap();
    let report = runner::run(plan, |_, _| async { Ok(json!({"symbol":["A","B"]})) }).await;
    assert_eq!(report["executed_calls"], 1);
    assert_eq!(report["steps"][1]["status"], "error");
}

#[tokio::test]
async fn context_budget_keeps_small_outputs_when_a_larger_section_does_not_fit() {
    let plan = plan::Plan::parse(
        json!({"max_bytes":4096,"outputs":["small","large"],"steps":[
            {"id":"small","tool":"code_hover"},{"id":"large","tool":"code_definition"}
        ]}),
    )
    .unwrap();
    let report = runner::run(plan, |tool, _| async move {
        Ok(json!({"text":if tool == "code_hover" {"kept".into()} else {"x".repeat(6000)}}))
    })
    .await;
    assert_eq!(report["steps"][0]["value"]["text"], "kept");
    assert!(report["steps"][1].get("value").is_none());
    assert_eq!(report["truncated"], true);
}

#[test]
fn slice_short_form_reserves_budget_for_sibling_sections_and_json() {
    let expanded = short_form::expand(
        json!({"symbol":"one","include":["body","dependencies"],"max_bytes":24000}),
    )
    .unwrap();
    let plan = plan::Plan::parse(expanded).unwrap();
    assert!(plan.steps[2].args["max_bytes"].as_u64().unwrap() <= 6000);
    let default = plan::Plan::parse(short_form::expand(json!({"symbol":"one"})).unwrap()).unwrap();
    assert_eq!(
        default.steps.len(),
        2,
        "the slice already includes the seed body"
    );
    assert_eq!(default.outputs, ["dependencies"]);
}

#[tokio::test]
async fn unexpanded_search_lines_have_an_explicit_utf8_safe_budget() {
    let root = tempfile::tempdir().unwrap();
    let source = format!("const BIG: &str = \"🙂needle{}\";\n", "🙂".repeat(5000));
    std::fs::write(root.path().join("sample.rs"), &source).unwrap();
    let report = search::grep(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        json!({"pattern":"needle","path":"sample.rs"}),
    )
    .await
    .unwrap();
    let text = report["unexpanded"][0]["text"].as_str().unwrap();
    assert!(text.len() <= 256);
    assert_eq!(report["unexpanded"][0]["text_truncated"], true);
    assert_eq!(
        report["unexpanded"][0]["column_bytes"],
        source.find("needle").unwrap() + 1
    );
    assert_eq!(report["unexpanded_total"], 1);
    assert_eq!(report["truncated"], true);
}

#[tokio::test]
async fn default_wire_result_has_one_full_report_and_structured_output_is_opt_in() {
    let root = tempfile::tempdir().unwrap();
    let result = execute(
        "127.0.0.1:1".parse().unwrap(),
        root.path(),
        json!({"steps":[{
            "id":"read","tool":"code_outline","args":{"path":"../outside.rs"}
        }]}),
    )
    .await
    .unwrap();
    assert!(result.structured_content.is_none());
    let McpContentItem::Text { text } = &result.content[0];
    let report: Value = serde_json::from_str(text).unwrap();
    assert_eq!(report["status"], "partial");
}

#[cfg(unix)]
#[test]
fn symbol_addresses_normalize_in_workspace_aliases_and_refuse_external_targets() {
    use std::os::unix::fs::symlink;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("ws");
    std::fs::create_dir(&root).unwrap();
    let root = std::fs::canonicalize(root).unwrap();
    std::fs::write(root.join("sample.rs"), "fn sample() {}\n").unwrap();
    let alias = temp.path().join("alias");
    symlink(&root, &alias).unwrap();
    let hit = |path| symbols::SymbolHit {
        path,
        name: "sample".into(),
        kind: "function",
        container: None,
        line: 1,
        col: 4,
    };
    assert_eq!(
        hit_value(&root, hit(alias.join("sample.rs"))).unwrap()["path"],
        "sample.rs"
    );
    let outside = temp.path().join("outside.rs");
    std::fs::write(&outside, "fn sample() {}\n").unwrap();
    symlink(&outside, root.join("escape.rs")).unwrap();
    assert!(hit_value(&root, hit(root.join("escape.rs"))).is_err());
}

#[test]
fn schema_defaults_materialize_into_valid_requests_in_every_mode() {
    let schema = tool_schema().input_schema;
    for mut request in [
        json!({"symbol":"one"}),
        json!({"grep":"needle"}),
        json!({"steps":[{"id":"one","tool":"code_outline"}]}),
    ] {
        for (key, property) in schema["properties"].as_object().unwrap() {
            if let Some(default) = property.get("default") {
                request
                    .as_object_mut()
                    .unwrap()
                    .entry(key.clone())
                    .or_insert(default.clone());
            }
        }
        assert!(
            short_form::expand(request.clone())
                .and_then(plan::Plan::parse)
                .is_ok(),
            "{request}"
        );
    }
}

#[test]
fn invalid_modes_and_bounds_are_rejected_before_running() {
    for request in [
        json!({"steps":null,"symbol":"one"}),
        json!({"symbol":"one","grep":"needle"}),
        json!({"symbol":"one","depth":4}),
        json!({"grep":""}),
        json!({"grep":"   "}),
    ] {
        assert!(short_form::expand(request).is_err());
    }
    for limit in [json!(0), json!(9), json!(-1), json!("eight")] {
        assert!(plan::Plan::parse(json!({"steps":[{"id":"find","tool":"symbols","args":{"query":"one","limit":limit}}]})).is_err());
    }
}
