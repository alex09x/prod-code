/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::*;
use prod_code_testkit::Workspace;
use serde_json::json;

mod dispatch_options;

#[tokio::test]
async fn dispatcher_preserves_required_argument_errors_for_tools_and_aliases() {
    let ws = Workspace::new(&[]);
    let remote = "127.0.0.1:1".parse().unwrap();
    for tool in [
        "code_safe_delete",
        "code_assists",
        "code_assist",
        "code_rename",
        "code_definition",
        "code_callers",
        "code_callees",
        "code_implementations",
        "code_supertypes",
        "code_extract_field",
        "code_inline_parameter",
        "code_introduce_variable",
        "code_extract_function",
        "code_loop_to_iterator",
        "code_extract_trait",
        "code_replace_constructor_with_factory",
        "code_replace_constructor_with_builder",
        "code_replace_constructor",
        "code_pull_up",
        "code_push_down",
        "code_replace_inheritance_with_delegation",
        "code_replace_inheritance",
        "code_replace_conditional_with_polymorphism",
        "code_replace_conditional",
        "code_extract_delegate",
        "code_extract_parameter",
        "code_move_method",
        "code_references",
        "code_hover",
        "code_type_at",
        "code_propose_expression",
    ] {
        let error = dispatch_tool(remote, &ws.root(), tool, json!({}))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Missing 'path' argument", "{tool}");
    }
    for (tool, expected) in [
        ("code_symbols", "Missing 'query' argument"),
        (
            "code_migrate_type",
            "Missing 'to' argument: the type it should become",
        ),
        ("code_structural_search", "Missing 'pattern' argument"),
        ("code_extract_interface", "Missing 'symbol' argument"),
        (
            "code_encapsulate_field",
            "Missing 'path' argument (or `symbol`)",
        ),
        ("code_make_static", "Missing 'path' argument (or `symbol`)"),
        (
            "code_convert_to_method",
            "Missing 'path' argument (or `symbol`)",
        ),
        (
            "code_wrap_return",
            "Missing 'wrapper' argument: `option`, `result`, `promise`, `pointer`, or custom envelope",
        ),
        (
            "code_generify",
            "Missing 'param' argument: the parameter to make generic",
        ),
        (
            "code_invert_boolean",
            "Missing 'path' argument (or `symbol`)",
        ),
        (
            "code_introduce_parameter_object",
            "Missing 'path' argument (or `symbol`)",
        ),
        ("code_move", "Missing 'path' argument (or `symbol`)"),
        (
            "code_change_signature",
            "Missing 'path' argument (or `symbol`)",
        ),
        (
            "code_move_module",
            "Missing 'path' argument: the module's file",
        ),
        (
            "code_diagnostics",
            "Missing 'path' argument (expected `path: \"...\"`)",
        ),
        (
            "code_validate_edit",
            "Missing 'path' argument (expected `path: \"...\"`)",
        ),
    ] {
        assert_eq!(
            dispatch_tool(remote, &ws.root(), tool, json!({}))
                .await
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    let unknown = dispatch_tool(remote, &ws.root(), "code_unknown", json!({}))
        .await
        .unwrap();
    assert!(unknown.is_error);
    let crate::protocol::McpContentItem::Text { text } = &unknown.content[0];
    assert_eq!(text, "Unknown tool: code_unknown");
    let builder = dispatch_tool(
        remote,
        &ws.root(),
        "code_replace_constructor",
        json!({"mode":"builder"}),
    )
    .await
    .unwrap_err();
    assert_eq!(builder.to_string(), "Missing 'path' argument");
}
