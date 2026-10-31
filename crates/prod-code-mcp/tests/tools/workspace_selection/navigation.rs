/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::fixture::{DivergentWorktrees, dual_worktree_gateway};
use super::*;
use prod_code_mcp::tools::WORKSPACE_SELECTABLE;

#[tokio::test]
async fn navigation_and_grep_tools_select_divergent_sibling_worktrees_and_preserve_defaults() {
    let advertised = prod_code_mcp::tools::list_tools();
    for name in WORKSPACE_SELECTABLE {
        let tool = advertised.iter().find(|t| t.name == *name).unwrap();
        assert_eq!(
            tool.input_schema["properties"]["workspace"]["type"], "string",
            "{name} schema must advertise string workspace"
        );
    }

    let trees = DivergentWorktrees::create();
    let (remote, seen_roots) =
        dual_worktree_gateway(trees.primary.clone(), trees.sibling.clone()).await;
    let primary_str = trees.primary.to_string_lossy().into_owned();
    let sibling_str = trees.sibling.to_string_lossy().into_owned();

    // Symbol-only and omitted-path dispatch across default (primary), explicit sibling, and explicit primary.
    let cases: &[(&str, serde_json::Value, &str, &str)] = &[
        (
            "code_definition",
            serde_json::json!({ "symbol": "target_fn" }),
            "helper_primary",
            "helper_sibling",
        ),
        (
            "code_symbols",
            serde_json::json!({ "query": "helper_" }),
            "helper_primary",
            "helper_sibling",
        ),
        (
            "code_grep",
            serde_json::json!({ "pattern": "helper_", "fixed_strings": true }),
            "helper_primary",
            "helper_sibling",
        ),
        (
            "code_references",
            serde_json::json!({ "symbol": "target_fn" }),
            "caller_primary",
            "caller_sibling",
        ),
        (
            "code_callers",
            serde_json::json!({ "symbol": "target_fn" }),
            "caller_primary",
            "caller_sibling",
        ),
        (
            "code_callees",
            serde_json::json!({ "symbol": "target_fn" }),
            "helper_primary",
            "helper_sibling",
        ),
        (
            "code_hover",
            serde_json::json!({ "symbol": "target_fn" }),
            "PrimaryReturn",
            "SiblingReturn",
        ),
        (
            "code_type_at",
            serde_json::json!({ "symbol": "target_fn" }),
            "PrimaryReturn",
            "SiblingReturn",
        ),
        (
            "code_implementations",
            serde_json::json!({ "symbol": "TargetTrait" }),
            "PrimaryImpl",
            "SiblingImpl",
        ),
        (
            "code_supertypes",
            serde_json::json!({ "symbol": "TargetTrait" }),
            "PrimarySuper",
            "SiblingSuper",
        ),
    ];
    for (tool, base_args, primary_marker, sibling_marker) in cases {
        let default_res = execute_tool(remote, &trees.ws.root(), tool, base_args.clone())
            .await
            .unwrap_or_else(|e| panic!("{tool} default failed: {e:#}"));
        let default_text = text_of(&default_res);
        assert!(
            !default_res.is_error,
            "{tool} default error: {default_text}"
        );
        assert!(
            default_text.contains(primary_marker),
            "{tool} default: {default_text}"
        );
        assert!(
            !default_text.contains(sibling_marker),
            "{tool} leaked sibling: {default_text}"
        );

        let mut sib_args = base_args.clone();
        sib_args["workspace"] = serde_json::json!(sibling_str);
        let sib_res = execute_tool(remote, &trees.ws.root(), tool, sib_args)
            .await
            .unwrap_or_else(|e| panic!("{tool} sibling failed: {e:#}"));
        let sib_text = text_of(&sib_res);
        assert!(!sib_res.is_error, "{tool} sibling error: {sib_text}");
        assert!(
            sib_text.contains(sibling_marker),
            "{tool} sibling: {sib_text}"
        );
        assert!(
            !sib_text.contains(primary_marker),
            "{tool} kept primary: {sib_text}"
        );

        let mut pri_args = base_args.clone();
        pri_args["workspace"] = serde_json::json!(primary_str);
        let pri_res = execute_tool(remote, &trees.ws.root(), tool, pri_args)
            .await
            .unwrap_or_else(|e| panic!("{tool} explicit primary failed: {e:#}"));
        assert!(
            text_of(&pri_res).contains(primary_marker),
            "{tool}: {}",
            text_of(&pri_res)
        );
    }

    // Sibling-only symbol, batch symbols, relative paths (file + directory), and also_in with workspace.
    let sib_only = execute_tool(
        remote,
        &trees.ws.root(),
        "code_definition",
        serde_json::json!({ "workspace": sibling_str, "symbol": "sibling_only_fn" }),
    )
    .await
    .expect("sibling_only_fn resolves in sibling");
    assert!(
        text_of(&sib_only).contains("sibling-worktree-canary"),
        "{}",
        text_of(&sib_only)
    );

    let batch = execute_tool(
        remote,
        &trees.ws.root(),
        "code_definition",
        serde_json::json!({
            "workspace": sibling_str,
            "symbols": ["target_fn", "sibling_only_fn"]
        }),
    )
    .await
    .expect("batch symbols resolve in sibling");
    assert!(
        text_of(&batch).contains("helper_sibling")
            && text_of(&batch).contains("sibling-worktree-canary")
    );

    for (tool, rel_args, primary_marker, sibling_marker) in [
        (
            "code_outline",
            serde_json::json!({ "path": "src/lib.rs" }),
            "PrimaryImpl",
            "SiblingImpl",
        ),
        (
            "code_outline",
            serde_json::json!({ "path": "src" }),
            "caller_primary",
            "sibling_only_fn",
        ),
        (
            "code_definition",
            serde_json::json!({ "path": "src/lib.rs", "line": 5, "character": 8 }),
            "helper_primary",
            "helper_sibling",
        ),
        (
            "code_grep",
            serde_json::json!({ "pattern": "fn ", "path": "src/lib.rs", "fixed_strings": true }),
            "caller_primary",
            "sibling_only_fn",
        ),
        (
            "code_symbols",
            serde_json::json!({ "query": "Impl", "path": "src/lib.rs" }),
            "PrimaryImpl",
            "SiblingImpl",
        ),
        (
            "code_hover",
            serde_json::json!({ "path": "src/lib.rs", "line": 5, "character": 8 }),
            "PrimaryReturn",
            "SiblingReturn",
        ),
    ] {
        let def_out = execute_tool(remote, &trees.ws.root(), tool, rel_args.clone())
            .await
            .unwrap();
        assert!(
            text_of(&def_out).contains(primary_marker),
            "{tool} rel default: {}",
            text_of(&def_out)
        );
        let mut sib_rel = rel_args;
        sib_rel["workspace"] = serde_json::json!(sibling_str);
        let sib_out = execute_tool(remote, &trees.ws.root(), tool, sib_rel)
            .await
            .unwrap();
        assert!(
            text_of(&sib_out).contains(sibling_marker),
            "{tool} rel sibling: {}",
            text_of(&sib_out)
        );
    }

    let across = execute_tool(
        remote,
        &trees.ws.root(),
        "code_references",
        serde_json::json!({
            "workspace": sibling_str,
            "symbol": "target_fn",
            "path": "src/lib.rs",
            "also_in": [primary_str]
        }),
    )
    .await
    .expect("references across sibling and primary succeeds");
    assert!(
        text_of(&across).contains("caller_sibling") && text_of(&across).contains("caller_primary"),
        "{}",
        text_of(&across)
    );

    let logged = seen_roots.lock().unwrap().clone();
    assert!(
        logged.contains(&primary_str) && logged.contains(&sibling_str),
        "{logged:?}"
    );
    trees.cleanup();
}
