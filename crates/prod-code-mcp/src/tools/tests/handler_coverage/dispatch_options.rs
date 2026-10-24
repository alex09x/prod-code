/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::tools::dispatch_tool;
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use serde_json::json;

#[tokio::test]
async fn pruning_an_empty_checkout_honors_write_patch_and_commit_flags() {
    for args in [
        json!({}),
        json!({"patch":true}),
        json!({"git_patch":true,"apply":true,"force":true,"reachability":true,"max_files":1}),
        json!({"commit":true}),
    ] {
        let ws = Workspace::new(&[(
            "Cargo.toml",
            "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )]);
        let gw = ScriptedGateway::start(|_, _| json!(null)).await;
        let result = dispatch_tool(gw.addr(), &ws.root(), "code_prune_orphans", args)
            .await
            .unwrap();
        assert!(!result.is_error);
        let prod_code_mcp_text = &result.content[0];
        let crate::protocol::McpContentItem::Text { text } = prod_code_mcp_text;
        assert_eq!(
            text,
            "0 orphan(s) of 0 symbol(s) checked\n\n\nnothing to prune\n"
        );
        assert_eq!(gw.calls(), 0);
    }
}

#[tokio::test]
async fn loop_dispatch_passes_aliases_positions_and_write_decisions() {
    let source = "export function total(prices: number[]): number {\n    let sum = 0;\n    for (const p of prices) {\n        sum += p * 2;\n    }\n    return sum;\n}\n";
    for apply in [false, true] {
        let ws = Workspace::new(&[("sample.ts", source)]);
        let gw = ScriptedGateway::start(|method, _| {
            if method == "textDocument/diagnostic" {
                answers::no_diagnostics()
            } else {
                json!(null)
            }
        })
        .await;
        let result = dispatch_tool(
            gw.addr(),
            &ws.root(),
            "code_loop_to_iterator",
            json!({
                "file":"sample.ts","function":"total","line":1,"col":17,
                "apply":apply,"force":false
            }),
        )
        .await
        .unwrap();
        assert!(!result.is_error);
        assert_eq!(
            ws.read("sample.ts"),
            if apply {
                "export function total(prices: number[]): number {\n    const sum = prices.reduce((acc, p) => acc + (p * 2), 0);\n    return sum;\n}\n"
            } else {
                source
            }
        );
        let crate::protocol::McpContentItem::Text { text } = &result.content[0];
        assert_eq!(
            text.lines().next(),
            Some("const sum = prices.reduce((acc, p) => acc + (p * 2), 0);")
        );
        assert_eq!(
            text.lines().last(),
            Some(if apply {
                "[applied]"
            } else {
                "nothing was written; pass `apply: true` to make this edit"
            })
        );
    }
}

#[tokio::test]
async fn issue_dispatch_dry_run_and_invalid_labels_never_publish() {
    let ws = Workspace::new(&[]);
    let gw = ScriptedGateway::start(|_, _| json!(null)).await;
    let result = dispatch_tool(gw.addr(), &ws.root(), "code_report_issue", json!({
        "title":"Analyzer result is incomplete", "body":"Ran a declaration query on a sample module. It returned an empty list; the expected result contains the declared function.",
        "dry_run":true, "force":true, "allow_private_names":true,
        "private_ref":"sample-reference", "labels":["bug",42,"mcp"]
    })).await.unwrap();
    assert!(!result.is_error, "{result:?}");
    let crate::protocol::McpContentItem::Text { text } = &result.content[0];
    assert_eq!(
        text.lines().take(4).collect::<Vec<_>>(),
        vec![
            "Dry run, nothing was filed. The issue would be:",
            "",
            "# Analyzer result is incomplete",
            "Labels: bug, mcp"
        ]
    );
    assert!(text.contains("Private details: report `sample-reference`."));
    let result = dispatch_tool(
        gw.addr(),
        &ws.root(),
        "code_report_issue",
        json!({"dry_run":true,"labels":["invalid"]}),
    )
    .await
    .unwrap();
    assert!(result.is_error);
    let crate::protocol::McpContentItem::Text { text } = &result.content[0];
    assert_eq!(
        text,
        "unsupported report label `invalid`: give one type (bug, enhancement, documentation, perf) and the areas the issue is about (gateway, client, mcp, cluster, worktree, infra, test), optionally with `roadmap`"
    );
}
