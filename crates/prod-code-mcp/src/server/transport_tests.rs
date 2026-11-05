/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#[test]
fn serialized_frame_bounding_with_escape_heavy_output() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    // 300,000 newlines escape to 600,000 bytes in JSON ("\n" -> "\\n")
    let raw = "\n".repeat(300_000);
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "content": [{
                "type": "text",
                "text": raw
            }],
            "isError": false
        }
    });

    let serialized = bound_serialized_response(response, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.contains("output truncated"));
}

#[test]
fn serialized_frame_bounding_returns_error_for_oversized_non_tool_responses() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    // An oversized tools/list response cannot be represented as a bounded tool content result.
    let big_tools: Vec<serde_json::Value> = (0..1500)
        .map(|i| {
            serde_json::json!({
                "name": format!("tool_{i}"),
                "description": "x".repeat(400),
                "inputSchema": { "type": "object" }
            })
        })
        .collect();

    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": big_tools
        }
    });

    let serialized = bound_serialized_response(resp.clone(), MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    let bounded: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(bounded["jsonrpc"], "2.0");
    assert_eq!(bounded["id"], 1);
    assert_eq!(bounded["error"]["code"], -32000);
    assert_eq!(
        bounded["error"]["message"],
        "Response exceeds maximum JSON-RPC frame size"
    );
}

#[test]
fn serialized_frame_bounding_falls_back_to_null_id_when_request_id_is_oversized() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};

    let big_tools: Vec<serde_json::Value> = (0..1500)
        .map(|i| {
            serde_json::json!({
                "name": format!("tool_{i}"),
                "description": "x".repeat(400),
                "inputSchema": { "type": "object" }
            })
        })
        .collect();

    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "a".repeat(MAX_JSONRPC_FRAME_BYTES * 2),
        "result": {
            "tools": big_tools
        }
    });

    let serialized = bound_serialized_response(resp, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    let bounded: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(bounded["jsonrpc"], "2.0");
    assert_eq!(bounded["id"], serde_json::Value::Null);
    assert_eq!(bounded["error"]["code"], -32000);
}

#[test]
fn real_tools_list_fits_comfortably_within_max_jsonrpc_frame_bytes() {
    use crate::server::transport::{MAX_JSONRPC_FRAME_BYTES, bound_serialized_response};
    use crate::tools::list_tools;

    let tools = list_tools();
    let resp = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "tools": tools
        }
    });

    let serialized = bound_serialized_response(resp, MAX_JSONRPC_FRAME_BYTES);
    assert!(serialized.len() <= MAX_JSONRPC_FRAME_BYTES);
    let val: serde_json::Value = serde_json::from_str(&serialized).unwrap();
    assert_eq!(val["jsonrpc"], "2.0");
    assert_eq!(val["id"], 1);
    assert!(val.get("result").is_some());
    assert!(val.get("error").is_none());
}

#[test]
fn navigation_tools_are_advertised_as_read_only_in_the_mcp_tool_catalog() {
    let tools = super::dispatch::list_tool_values().unwrap();
    for name in [
        "code_outline",
        "code_definition",
        "code_callers",
        "code_references",
        "code_source",
    ] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
        assert_eq!(tool["annotations"]["destructiveHint"], false, "{name}");
        assert_eq!(tool["annotations"]["idempotentHint"], true, "{name}");
        assert_eq!(tool["annotations"]["openWorldHint"], false, "{name}");
    }
    let mutation = tools
        .iter()
        .find(|tool| tool["name"] == "code_rename")
        .unwrap();
    assert!(mutation.get("annotations").is_none());
}

/// A host with approval policy `never` refuses any tool not marked read-only (#1887, #1890,
/// #1905). Every tool the server treats as read-only whatever its arguments says so in the
/// catalog, and nothing is marked read-only that the server would let write or run a command.
#[test]
fn the_catalog_marks_exactly_the_tools_the_server_treats_as_read_only() {
    use crate::server::dispatch::READ_ONLY_NAVIGATION;
    let empty = serde_json::json!({});
    for name in READ_ONLY_NAVIGATION {
        assert!(
            crate::tools::execution::is_read_only_tool(name, &empty),
            "{name} is advertised as read-only but the server does not treat it so"
        );
    }
    for tool in super::dispatch::list_tool_values().unwrap() {
        let name = tool["name"].as_str().unwrap().to_string();
        let advertised = tool.get("annotations").is_some();
        assert_eq!(
            advertised,
            READ_ONLY_NAVIGATION.contains(&name.as_str()),
            "{name}: annotations must follow the list"
        );
    }
    for name in [
        "code_impact",
        "code_diagnostics",
        "code_dependencies",
        "code_status",
        "code_assists",
    ] {
        let tools = super::dispatch::list_tool_values().unwrap();
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert_eq!(tool["annotations"]["readOnlyHint"], true, "{name}");
    }
    for name in [
        "code_exec",
        "code_pull",
        "code_check",
        "code_test",
        "code_report_issue",
        "code_workflow",
    ] {
        let tools = super::dispatch::list_tool_values().unwrap();
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        assert!(tool.get("annotations").is_none(), "{name} can write or run");
    }
}

#[tokio::test]
async fn cluster_rebalance_tick_ignores_filesystem_root() {
    let mut remote: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let initial_remote = remote;
    crate::server::rebalance::handle_cluster_rebalance_tick(&mut remote, std::path::Path::new("/"))
        .await;
    assert_eq!(
        remote, initial_remote,
        "filesystem root must never trigger rebalance"
    );
}

#[tokio::test]
async fn cluster_rebalance_tick_ignores_unbounded_non_git_manifestless_dir() {
    let temp = tempfile::tempdir().unwrap();
    let mut remote: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let initial_remote = remote;
    crate::server::rebalance::handle_cluster_rebalance_tick(&mut remote, temp.path()).await;
    assert_eq!(
        remote, initial_remote,
        "manifestless non-git root must not trigger rebalance"
    );
}

#[tokio::test]
async fn cluster_rebalance_tick_skips_cgo_check_on_rust_workspace() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("Cargo.toml"),
        "[package]\nname = \"dummy\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let target_dir = temp.path().join("target/debug");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::write(
        target_dir.join("proc.go"),
        "package proc\n// #include <libproc.h>\nimport \"C\"\n",
    )
    .unwrap();

    // Directly verify that macos_only_cgo ignores the target/ directory
    assert_eq!(crate::sync::macos_only_cgo(temp.path()), None);

    // Verify detected engine is rust, so rebalance tick passes os = None
    let (_, engine) = crate::sync::engine_project(temp.path(), temp.path());
    assert_eq!(engine, Some("rust"));
}

#[tokio::test]
async fn workspace_mutation_lock_allows_concurrent_reads_and_serializes_mutations() {
    use crate::tools::execution::{is_read_only_tool, workspace_mutation_lock};

    assert!(is_read_only_tool("code_definition", &serde_json::json!({})));
    assert!(is_read_only_tool("code_references", &serde_json::json!({})));
    assert!(is_read_only_tool(
        "code_shadow_run",
        &serde_json::json!({"argv": ["cargo", "test"], "hypotheses": []})
    ));
    assert!(!is_read_only_tool(
        "code_shadow_run",
        &serde_json::json!({"argv": ["cargo", "test"], "hypotheses": [], "apply": true})
    ));
    assert!(is_read_only_tool("code_lint", &serde_json::json!({})));
    assert!(is_read_only_tool(
        "code_pull",
        &serde_json::json!({"list": true})
    ));
    assert!(!is_read_only_tool("code_pull", &serde_json::json!({})));
    assert!(!is_read_only_tool(
        "code_lint",
        &serde_json::json!({"fix": true})
    ));
    assert!(!is_read_only_tool(
        "code_check",
        &serde_json::json!({"fix": true})
    ));
    assert!(!is_read_only_tool(
        "code_exec",
        &serde_json::json!({"argv": ["cargo", "test"]})
    ));
    assert!(!is_read_only_tool(
        "code_change_signature",
        &serde_json::json!({"apply": true})
    ));
    assert!(!is_read_only_tool("code_rename", &serde_json::json!({})));

    let temp = tempfile::tempdir().unwrap();
    let lock = workspace_mutation_lock(temp.path());
    let r1 = lock.clone().try_read_owned().expect("first read lock");
    let r2 = lock
        .clone()
        .try_read_owned()
        .expect("second concurrent read lock");
    assert!(
        lock.clone().try_write_owned().is_err(),
        "mutating write lock must wait while read locks are held"
    );
    drop(r1);
    drop(r2);
    let w1 = lock
        .clone()
        .try_write_owned()
        .expect("write lock after reads finish");
    assert!(
        lock.clone().try_write_owned().is_err(),
        "second mutating tool must serialize behind active write lock"
    );
    assert!(
        lock.clone().try_read_owned().is_err(),
        "read lock must wait while mutating tool holds write lock"
    );
    drop(w1);
}
