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
use super::common::peer;
use prod_code_protocol::{LoadedWorkspaceInfo, PlaceRequest};

/// An editor is offered what the language server on the node offers, but always asked for
/// whole documents, which is what the gateway hands on; the Rust engine offers what it
/// answers (#310).
#[test]
fn an_editor_gets_the_servers_capabilities_and_sends_whole_documents() {
    let gopls = serde_json::json!({
        "textDocumentSync": { "openClose": true, "change": 2, "save": {} },
        "completionProvider": { "triggerCharacters": ["."] },
        "hoverProvider": true
    });
    let caps = editor_capabilities(Some(gopls), false);
    assert_eq!(
        caps["textDocumentSync"],
        serde_json::json!({ "openClose": true, "change": 1, "save": {} })
    );
    assert_eq!(
        caps["completionProvider"]["triggerCharacters"],
        serde_json::json!(["."])
    );
    assert_eq!(caps["hoverProvider"], true);

    let numeric = editor_capabilities(Some(serde_json::json!({ "textDocumentSync": 2 })), false);
    assert_eq!(
        numeric["textDocumentSync"],
        serde_json::json!({ "openClose": true, "change": 1 })
    );

    let rust = editor_capabilities(None, true);
    assert_eq!(rust["renameProvider"], true);
    assert_eq!(rust["callHierarchyProvider"], true);
    assert_eq!(rust["completionProvider"]["resolveProvider"], true);
    assert_eq!(rust["codeActionProvider"]["resolveProvider"], true);
    assert_eq!(rust["textDocumentSync"]["change"], 1);
    // Every method advertised for Rust is one the engine answers.
    for (capability, method) in [
        ("completionProvider", "textDocument/completion"),
        ("signatureHelpProvider", "textDocument/signatureHelp"),
        ("inlayHintProvider", "textDocument/inlayHint"),
        (
            "documentHighlightProvider",
            "textDocument/documentHighlight",
        ),
        ("codeActionProvider", "textDocument/codeAction"),
        ("documentFormattingProvider", "textDocument/formatting"),
    ] {
        assert!(rust.get(capability).is_some(), "{capability}");
        assert!(
            prod_code_engine_rust::editor::EDITOR_METHODS.contains(&method),
            "{method}"
        );
    }

    assert_eq!(
        editor_capabilities(None, false),
        serde_json::json!({ "textDocumentSync": { "openClose": true, "change": 1 } })
    );
}

/// A macOS node is a developer's Mac: it takes work that needs macOS, or that no other live
/// node serves, and nothing else, however quiet it is (#308).
#[test]
fn a_macos_node_takes_only_what_needs_macos_or_what_nothing_else_serves() {
    let view = ClusterResponse {
        this_node: "linux:9400".to_string(),
        nodes: vec![
            peer(
                "linux:9400",
                "linux x86_64",
                &["rust (ra_ap_ide)", "go (gopls)"],
                0.9,
            ),
            peer(
                "mac:9400",
                "macos aarch64",
                &["swift (sourcekit-lsp)", "go (gopls)"],
                0.01,
            ),
        ],
    };
    let place = |view: &ClusterResponse, engine: Option<&str>, os: Option<&str>| {
        place_in(
            &PlaceRequest {
                workspace_name: "subject".to_string(),
                engine: engine.map(str::to_string),
                os: os.map(str::to_string),
                rebalance_active: false,
            },
            view.clone(),
        )
        .node
    };
    assert_eq!(
        place(&view, Some("go"), None).as_deref(),
        Some("linux:9400"),
        "plain Go stays on Linux though the Mac is far quieter"
    );
    assert_eq!(
        place(&view, None, None).as_deref(),
        Some("linux:9400"),
        "so does a workspace whose engine is unknown"
    );
    assert_eq!(
        place(&view, Some("go"), Some("macos")).as_deref(),
        Some("mac:9400"),
        "Go with macOS-only cgo goes to the Mac"
    );
    assert_eq!(
        place(&view, Some("swift"), None).as_deref(),
        Some("mac:9400"),
        "only the Mac serves Swift"
    );

    // A workspace already on the Mac that does not need macOS moves to Linux.
    let mut held = view.clone();
    held.nodes[1].workspaces.push(LoadedWorkspaceInfo {
        name: "subject".to_string(),
        engine: "go".to_string(),
        sessions: 0,
    });
    assert_eq!(
        place(&held, Some("go"), None).as_deref(),
        Some("linux:9400")
    );

    // With the Linux node down, the Mac takes plain Go rather than nothing.
    let mut down = view.clone();
    down.nodes[0].alive = false;
    assert_eq!(place(&down, Some("go"), None).as_deref(), Some("mac:9400"));
}

#[test]
fn active_workspace_on_pressured_or_congested_holder_moves_to_roomy_quiet_node() {
    let mut view = ClusterResponse {
        this_node: "node1:9400".to_string(),
        nodes: vec![
            peer("node1:9400", "linux x86_64", &["rust (ra_ap_ide)"], 1.8),
            peer("node2:9400", "linux x86_64", &["rust (ra_ap_ide)"], 0.2),
        ],
    };
    // node1 holds active sessions for "subject"
    view.nodes[0].workspaces.push(LoadedWorkspaceInfo {
        name: "subject".to_string(),
        engine: "rust".to_string(),
        sessions: 5,
    });

    let place_req = |v: &ClusterResponse| {
        place_in(
            &PlaceRequest {
                workspace_name: "subject".to_string(),
                engine: Some("rust".to_string()),
                os: None,
                rebalance_active: false,
            },
            v.clone(),
        )
    };

    // 1. Congestion rebalance: score 1.8 vs 0.2 moves to node2 even with active sessions
    let resp = place_req(&view);
    assert_eq!(resp.node.as_deref(), Some("node2:9400"));
    assert!(resp.reason.contains("rebalanced from"));

    // 2. Resource pressure: node1 has memory pressure (> 85% used)
    view.nodes[0].status.load_average_millis = Some(400); // low load but hard memory pressure
    view.nodes[0].status.host.memory_total_bytes = Some(100 * 1024 * 1024);
    view.nodes[0].status.host.memory_available_bytes = Some(10 * 1024 * 1024); // 90% used
    let resp = place_req(&view);
    assert_eq!(resp.node.as_deref(), Some("node2:9400"));
    assert!(resp.reason.contains("moved from"));
}
