/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Explicitly invoked comparison on an existing gateway, using only synthetic source.
use prod_code_mcp::{
    protocol::{McpContentItem, McpToolCallResult},
    tools::execute_tool,
};
use serde_json::{Value, json};
use std::{net::SocketAddr, time::Instant};

fn text(result: &McpToolCallResult) -> String {
    result
        .content
        .iter()
        .map(|item| match item {
            McpContentItem::Text { text } => text.as_str(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn setup() -> (tempfile::TempDir, SocketAddr) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("src")).unwrap();
    std::fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname=\"workflow_probe\"\nversion=\"0.1.0\"\nedition=\"2024\"\n",
    )
    .unwrap();
    // Cargo metadata creates this file on the gateway. Keeping it in the fixture prevents
    // the next mirror pass deleting it and forcing another project-model reload.
    std::fs::write(
        root.path().join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"workflow_probe\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    std::fs::write(root.path().join("src/lib.rs"), "pub fn workflow_unique(value: u32) -> u32 { helper(value) + 1 }\nfn helper(value: u32) -> u32 { value * 2 }\npub mod left { pub fn repeated() {} }\npub mod right { pub fn repeated() {} }\n").unwrap();
    let gateway = std::env::var("PROD_CODE_WORKFLOW_TEST_REMOTE")
        .expect("set PROD_CODE_WORKFLOW_TEST_REMOTE to an existing test gateway")
        .parse()
        .unwrap();
    (root, gateway)
}

#[tokio::test]
#[ignore = "requires an explicitly selected existing gateway"]
async fn workflow_live_matches_standalone_definition_and_slice() {
    let (root, remote) = setup();
    let started = Instant::now();
    let mut baseline = Vec::new();
    for (tool, args) in [
        (
            "code_symbols",
            json!({"query":"workflow_unique","path":"src/lib.rs"}),
        ),
        (
            "code_definition",
            json!({"path":"src/lib.rs","line":1,"character":8}),
        ),
        (
            "code_slice",
            json!({"symbol":"workflow_unique","path":"src/lib.rs","depth":1,"max_bytes":6000}),
        ),
    ] {
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(45),
            execute_tool(remote, root.path(), tool, args),
        )
        .await
        .expect("standalone operation exceeded 45 seconds")
        .unwrap();
        assert!(!result.is_error, "{tool}: {}", text(&result));
        baseline.push(text(&result));
    }
    let baseline_ms = started.elapsed().as_millis();
    let plan = json!({"outputs":["body","context"],"timeout_ms":60000,"steps":[
        {"id":"target","tool":"resolve","args":{"symbol":"workflow_unique","path":"src/lib.rs"}},
        {"id":"body","tool":"code_definition","args":{"path":{"$ref":"target#/path"},"line":{"$ref":"target#/line"},"character":{"$ref":"target#/character"}}},
        {"id":"context","tool":"code_slice","args":{"path":{"$ref":"target#/path"},"line":{"$ref":"target#/line"},"character":{"$ref":"target#/character"},"depth":1,"max_bytes":6000}}
    ]});
    let started = Instant::now();
    let result = execute_tool(remote, root.path(), "code_workflow", plan)
        .await
        .unwrap();
    let workflow_ms = started.elapsed().as_millis();
    let output = text(&result);
    let report: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(report["status"], "complete", "{output}");
    assert_eq!(report["steps"][1]["value"]["text"], baseline[1]);
    assert_eq!(report["steps"][2]["value"]["text"], baseline[2]);
    assert!(baseline[2].contains("helper"));
    println!(
        "workflow comparison (baseline runs first; not a controlled latency benchmark): standalone_calls=3 workflow_calls=1 internal_workflow_calls={} standalone_ms={baseline_ms} workflow_ms={workflow_ms} standalone_visible_bytes={} workflow_visible_bytes={}",
        report["executed_calls"],
        baseline.iter().map(String::len).sum::<usize>(),
        output.len()
    );
}

#[tokio::test]
#[ignore = "requires an explicitly selected existing gateway"]
async fn workflow_live_ambiguous_symbol_takes_explicit_recovery_branch() {
    let (root, remote) = setup();
    let plan = json!({"timeout_ms":60000,"outputs":["lookup","candidates"],"steps":[
        {"id":"lookup","tool":"resolve","args":{"symbol":"repeated","path":"src/lib.rs"},"on_error":"continue"},
        {"id":"never","tool":"code_definition","when":{"step":"lookup","status":"ok"},"args":{"path":{"$ref":"lookup#/path"},"line":{"$ref":"lookup#/line"},"character":{"$ref":"lookup#/character"}}},
        {"id":"candidates","tool":"symbols","when":{"step":"lookup","status":"error"},"args":{"query":"repeated","path":"src/lib.rs"}}
    ]});
    let result = execute_tool(remote, root.path(), "code_workflow", plan)
        .await
        .unwrap();
    let output = text(&result);
    let report: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(report["status"], "partial", "{output}");
    assert_eq!(report["steps"][0]["status"], "error", "{output}");
    assert_eq!(report["steps"][1]["status"], "skipped", "{output}");
    assert!(
        report["steps"][2]["value"]["items"]
            .as_array()
            .unwrap()
            .len()
            >= 2,
        "{output}"
    );
}
