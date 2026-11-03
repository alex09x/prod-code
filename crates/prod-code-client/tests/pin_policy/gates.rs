/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The pin gate on every CLI connection that syncs or can run work (#1793), and the blank pin
//! that must not shadow a workspace's own.

use super::*;

/// Frames that carry the workspace or start work; none may reach an unconfirmed gateway.
const WORK: &[&str] = &[
    "SyncProbeRequest",
    "SyncRequest",
    "HandshakeRequest",
    "ExecRequest",
    "SearchRequest",
    "PullFileRequest",
];

#[tokio::test]
async fn issue_1793_cli_sync_query_and_editor_send_nothing_to_an_old_gateway() {
    let ws = workspace(&[]);
    for args in [
        &["sync"][..],
        &["hover", "src/lib.rs", "1", "8"][..],
        &["lsp"][..],
    ] {
        let old = Node::start(Kind::Old, 0, None).await;
        let pin = old.addr.to_string();
        let mut full = vec!["--pin-node", pin.as_str()];
        full.extend_from_slice(args);
        let output = cli(&ws, &full, &[]).await;
        let seen = old.seen();
        assert!(
            seen.iter().any(|s| s == "NodePinRequest"),
            "{args:?} reached the gate: {seen:?}\n{}",
            err(&output)
        );
        for frame in WORK {
            assert!(
                !seen.iter().any(|s| s == frame),
                "{args:?} sent {frame} to a gateway that never confirmed the pin: {seen:?}"
            );
        }
        if args[0] != "lsp" {
            assert_ne!(output.status.code(), Some(0), "{args:?}");
            assert!(
                err(&output).contains("older than node pins"),
                "{}",
                err(&output)
            );
        }
    }
}

#[tokio::test]
async fn issue_1793_cli_sync_confirms_on_a_current_gateway_first() {
    let ws = workspace(&[]);
    let pinned = Node::start(Kind::Current, 0, None).await;
    let pin = pinned.addr.to_string();
    let output = cli(&ws, &["--pin-node", &pin, "sync"], &[]).await;
    assert_eq!(output.status.code(), Some(0), "{}", err(&output));
    let seen: Vec<String> = pinned
        .seen()
        .into_iter()
        .filter(|s| s != "connect" && s != "StatusRequest")
        .collect();
    assert_eq!(
        seen.first().map(String::as_str),
        Some("NodePinRequest"),
        "{seen:?}"
    );
    assert!(seen.iter().any(|s| s == "SyncProbeRequest"), "{seen:?}");
}

#[tokio::test]
async fn issue_1793_a_blank_pin_variable_keeps_the_workspace_pin() {
    let pinned = Node::start(Kind::Current, 0, None).await;
    let seed = Node::start(Kind::Current, 0, None).await;
    let config = format!("[cluster]\npin = \"{}\"\n", pinned.addr);
    let ws = workspace(&[("prod-code.toml", config.as_str())]);
    for blank in ["", "   "] {
        let output = cli(
            &ws,
            &["check", "--json"],
            &[
                ("PROD_CODE_PIN_NODE", blank.to_string()),
                ("PROD_CODE_REMOTE", seed.addr.to_string()),
            ],
        )
        .await;
        assert_eq!(output.status.code(), Some(0), "{blank:?}: {}", err(&output));
        let report: serde_json::Value = serde_json::from_str(&out(&output)).unwrap();
        assert_eq!(report["execution"]["node"], pinned.addr.to_string());
        assert_eq!(report["execution"]["gateway_node"], CONFIRMED_NODE);
        assert_eq!(report["executor"]["confirmed_node"], CONFIRMED_NODE);
        assert!(seed.seen().is_empty(), "{blank:?}: {:?}", seed.seen());
    }
}
