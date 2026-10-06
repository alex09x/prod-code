/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::super::search::{DenseStatus, SearchHit, SearchResponse};
use super::super::sync::{FileDelta, SyncRequest, SyncResponse};
use super::super::wire::{EngineKind, WireMessage};

/// Every message that crosses the wire has to survive the trip. A field added on one side
/// and missing on the other is the failure this catches: `#[serde(default)]` makes an old
/// message readable by a new binary, and that only holds while somebody checks.
#[test]
fn a_message_without_its_optional_fields_still_decodes() {
    let minimal = r#"{"client_workspace_root":"/w","files":[],"clean_others":false}"#;
    let request: SyncRequest = serde_json::from_str(minimal).expect("an older client's sync");
    assert_eq!(request.client_workspace_root, "/w");
    assert!(request.base_workspace_name.is_none());

    let delta = r#"{"relative_path":"a.rs"}"#;
    let file: FileDelta = serde_json::from_str(delta).expect("a deletion");
    assert!(
        file.content.is_none(),
        "no content is how a deletion is spelled"
    );
    assert!(!file.is_executable);
}

#[test]
fn a_wire_message_keeps_its_kind_through_a_round_trip() {
    let original = WireMessage::SyncResponse(SyncResponse {
        server_workspace_root: "/srv/w".to_string(),
        files_updated: 3,
        files_deleted: 1,
        bytes_transferred: 4096,
        duration_ms: 12,
        workspace_was_fresh: true,
        stale_paths: vec!["src/big.rs".to_string()],
    });
    let json = serde_json::to_string(&original).expect("encode");
    let back: WireMessage = serde_json::from_str(&json).expect("decode");
    match back {
        WireMessage::SyncResponse(response) => {
            assert_eq!(response.files_updated, 3);
            assert!(response.workspace_was_fresh);
        }
        other => panic!("a sync response came back as {other:?}"),
    }
}

#[test]
fn redirect_message_round_trips() {
    let msg = WireMessage::Redirect {
        target_addr: "192.168.2.191:9400".to_string(),
        reason: Some("engine loaded warm on peer".to_string()),
    };
    let encoded = serde_json::to_string(&msg).expect("encode redirect");
    let decoded: WireMessage = serde_json::from_str(&encoded).expect("decode redirect");
    assert_eq!(msg, decoded);
}

#[test]
fn polyglot_engine_kinds_round_trip() {
    let languages = [
        ("haskell", EngineKind::Haskell),
        ("ocaml", EngineKind::Ocaml),
        ("clojure", EngineKind::Clojure),
        ("julia", EngineKind::Julia),
        ("shell", EngineKind::Shell),
        ("r", EngineKind::R),
        ("erlang", EngineKind::Erlang),
        ("fsharp", EngineKind::Fsharp),
        ("perl", EngineKind::Perl),
        ("solidity", EngineKind::Solidity),
        ("nim", EngineKind::Nim),
        ("d", EngineKind::D),
        ("fortran", EngineKind::Fortran),
        ("sql", EngineKind::Sql),
        ("graphql", EngineKind::Graphql),
        ("protobuf", EngineKind::Protobuf),
        ("crystal", EngineKind::Crystal),
        ("groovy", EngineKind::Groovy),
        ("ada", EngineKind::Ada),
        ("v", EngineKind::V),
        ("racket", EngineKind::Racket),
        ("terraform", EngineKind::Terraform),
        ("nix", EngineKind::Nix),
        ("markdown", EngineKind::Markdown),
        ("yaml", EngineKind::Yaml),
        ("toml", EngineKind::Toml),
        ("json", EngineKind::Json),
        ("html", EngineKind::Html),
        ("css", EngineKind::Css),
        ("dockerfile", EngineKind::Dockerfile),
        ("svelte", EngineKind::Svelte),
        ("vue", EngineKind::Vue),
        ("assembly", EngineKind::Assembly),
        ("powershell", EngineKind::Powershell),
        ("starlark", EngineKind::Starlark),
        ("hcl", EngineKind::Hcl),
        ("typst", EngineKind::Typst),
        ("wat", EngineKind::Wat),
        ("systemverilog", EngineKind::SystemVerilog),
        ("vhdl", EngineKind::Vhdl),
        ("ballerina", EngineKind::Ballerina),
        ("jsonnet", EngineKind::Jsonnet),
        ("cue", EngineKind::Cue),
    ];
    for (name, kind) in languages {
        assert_eq!(kind.as_str(), name);
        assert_eq!(name.parse::<EngineKind>().unwrap(), kind);
    }
}

#[test]
fn search_response_survives_wire_trip_and_older_payloads() {
    let hit = SearchHit {
        file: "src/search.rs".into(),
        line: 42,
        kind: "struct".into(),
        name: "WorkspaceIndex".into(),
        container: None,
        signature: "pub struct WorkspaceIndex".into(),
        doc: "Primary workspace declaration index.".into(),
        score: Some("0.0385".into()),
        rank_reasons: Some(vec![
            "lexical: matched 'workspace'".into(),
            "graph: struct in-degree 12 (centrality 1.76)".into(),
        ]),
    };
    let response = SearchResponse {
        server_workspace_root: "/srv/w".into(),
        hits: vec![hit.clone()],
        indexed_files: 10,
        indexed_declarations: 200,
        took_ms: 3,
        error: None,
        dense: Some(DenseStatus {
            used: true,
            embedded: 200,
        }),
        graph_fused: Some(true),
    };
    let encoded = serde_json::to_string(&response).expect("serialize response");
    let decoded: SearchResponse = serde_json::from_str(&encoded).expect("deserialize response");
    assert_eq!(response, decoded);

    // Older payload without score, rank_reasons, or graph_fused still decodes cleanly
    let old_json = r#"{
        "server_workspace_root": "/srv/w",
        "hits": [{
            "file": "src/search.rs",
            "line": 42,
            "kind": "struct",
            "name": "WorkspaceIndex",
            "signature": "pub struct WorkspaceIndex",
            "doc": "Primary index."
        }],
        "indexed_files": 10,
        "indexed_declarations": 200,
        "took_ms": 3
    }"#;
    let old_decoded: SearchResponse = serde_json::from_str(old_json).expect("older payload");
    assert_eq!(old_decoded.hits[0].score, None);
    assert_eq!(old_decoded.hits[0].rank_reasons, None);
    assert_eq!(old_decoded.graph_fused, None);
}
