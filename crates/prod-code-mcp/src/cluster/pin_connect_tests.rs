/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Every connection that syncs a checkout or can run work passes the pin gate (#1793): under a
//! strict pin a gateway older than pins receives the pin request and nothing else, whichever
//! entry point opened the connection, and a current gateway's confirmation is what the reports
//! name.

use super::*;
use crate::cluster::pin_fake::{CONFIRMED, FakeGateway, Kind, checkout, strict};
use crate::cluster::with_root_policy;
use serde_json::json;
use std::path::PathBuf;

/// The error text of a tool call that failed, as an error or as an error result.
fn failure(result: Result<crate::protocol::McpToolCallResult>) -> String {
    match result {
        Err(err) => format!("{err:#}"),
        Ok(result) => serde_json::to_string(&result).unwrap(),
    }
}

/// Runs `work` on a checkout strictly pinned to a fresh old gateway; returns what it said and
/// the frames that gateway received.
async fn against_old<F, Fut>(work: F) -> (String, Vec<String>)
where
    F: FnOnce(SocketAddr, PathBuf) -> Fut,
    Fut: std::future::Future<Output = String>,
{
    let (_dir, root) = checkout(&[]);
    let old = FakeGateway::start(Kind::Old).await;
    let policy = strict(old.addr, "--pin-node");
    let said = with_root_policy(&root, policy, work(old.addr, root.clone())).await;
    (said, old.frames())
}

fn assert_refused_before_anything(entry: &str, said: &str, frames: &[String]) {
    assert!(
        said.contains("did not confirm the node pin"),
        "{entry}: {said}"
    );
    assert!(
        said.contains("no workspace sync or command was sent"),
        "{entry}: {said}"
    );
    assert_eq!(
        frames,
        ["NodePinRequest"],
        "{entry}: the old gateway saw only the pin request"
    );
}

#[tokio::test]
async fn issue_1793_session_search_shadow_sync_and_exec_confirm_before_any_frame() {
    let (said, frames) = against_old(|addr, root| async move {
        crate::session::LspSession::open(addr, &root, None)
            .await
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default()
    })
    .await;
    assert_refused_before_anything("LspSession::open", &said, &frames);

    let (said, frames) = against_old(|addr, root| async move {
        crate::search::search(addr, &root, "pinned function", 5, None)
            .await
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default()
    })
    .await;
    assert_refused_before_anything("search", &said, &frames);

    let (said, frames) = against_old(|addr, root| async move {
        let spec = crate::shadow::HypothesisSpec {
            name: "a".into(),
            edits: vec![crate::shadow::HypothesisEdit {
                relative_path: "src/lib.rs".into(),
                text: Some("pub fn pinned() {}\n".into()),
            }],
        };
        crate::shadow::execute::run_shadow_once(
            addr,
            &root,
            None,
            &[spec],
            vec!["true".into()],
            vec![],
            10,
            1,
            1024,
            false,
        )
        .await
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default()
    })
    .await;
    assert_refused_before_anything("shadow run", &said, &frames);

    let (said, frames) = against_old(|addr, root| async move {
        failure(crate::tools::execute_tool(addr, &root, "code_sync", json!({})).await)
    })
    .await;
    assert_refused_before_anything("code_sync", &said, &frames);

    let (said, frames) = against_old(|addr, root| async move {
        crate::exec::run_remote(
            addr,
            &root,
            None,
            vec!["true".into()],
            vec![],
            10,
            false,
            |_, _| {},
        )
        .await
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default()
    })
    .await;
    assert_refused_before_anything("exec", &said, &frames);
}

#[tokio::test]
async fn issue_1793_artifact_pull_and_list_confirm_before_any_request() {
    let (said, frames) = against_old(|addr, root| async move {
        crate::sync::pull_remote_files_from(
            &crate::cluster::PlacementStore::default(),
            addr,
            &root,
            &[PathBuf::from("target/out.bin")],
        )
        .await
        .err()
        .map(|e| format!("{e:#}"))
        .unwrap_or_default()
    })
    .await;
    assert_refused_before_anything("pull", &said, &frames);

    let (said, frames) = against_old(|addr, root| async move {
        crate::sync::list_remote_artifacts_from(addr, &root, std::path::Path::new("target"))
            .await
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default()
    })
    .await;
    assert_refused_before_anything("artifact list", &said, &frames);
}

#[tokio::test]
async fn issue_1793_a_node_outside_the_pin_is_never_connected_and_unpinned_is_unchanged() {
    let (_dir, root) = checkout(&[]);
    let pinned = FakeGateway::start(Kind::Current).await;
    let outside = FakeGateway::start(Kind::Current).await;
    let policy = strict(pinned.addr, "--pin-node");
    let err = connect_gated_with(outside.addr, &root, &policy, "this sync")
        .await
        .err()
        .expect("refused");
    assert!(format!("{err:#}").contains("refusing"), "{err:#}");
    assert!(outside.seen().is_empty(), "{:?}", outside.seen());

    // Without a strict pin an old gateway is used as before and never asked to pin.
    let old = FakeGateway::start(Kind::Old).await;
    let gated = connect_gated_with(old.addr, &root, &NodePolicy::default(), "this sync")
        .await
        .unwrap();
    assert_eq!(gated.confirmed_node, None);
    drop(gated);
    assert!(old.frames().is_empty(), "{:?}", old.frames());
}

#[tokio::test]
async fn issue_1793_reports_name_the_node_the_gateway_confirmed_on_that_connection() {
    let (_dir, root) = checkout(&[]);
    let pinned = FakeGateway::start(Kind::Current).await;
    let policy = strict(pinned.addr, "prod-code.toml [cluster] pin");
    let confirms = format!("node {} (gateway confirms {CONFIRMED})", pinned.addr);

    let report = with_root_policy(
        &root,
        policy.clone(),
        crate::verify::run_verify(
            pinned.addr,
            &root,
            None,
            crate::verify::VerifyKind::Check,
            None,
            10,
        ),
    )
    .await
    .unwrap();
    let executor = report
        .executor
        .clone()
        .expect("the report names its executor");
    assert_eq!(executor.node, pinned.addr.to_string());
    assert_eq!(executor.confirmed_node.as_deref(), Some(CONFIRMED));
    assert!(
        report.render(40).contains(&confirms),
        "{}",
        report.render(40)
    );
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["executor"]["confirmed_node"], CONFIRMED);
    let frames = pinned.frames();
    assert_eq!(frames.first().map(String::as_str), Some("NodePinRequest"));

    // The MCP check and exec results say the same.
    for (tool, args) in [
        ("code_check", json!({})),
        ("code_test", json!({})),
        ("code_exec", json!({ "argv": ["true"] })),
    ] {
        let result = with_root_policy(
            &root,
            policy.clone(),
            crate::tools::execute_tool(pinned.addr, &root, tool, args),
        )
        .await
        .unwrap();
        let text = serde_json::to_string(&result).unwrap();
        assert!(text.contains(&confirms), "{tool}: {text}");
    }

    // A later unpinned run on the same address does not inherit that confirmation.
    let unpinned = with_root_policy(
        &root,
        NodePolicy::default(),
        crate::verify::run_verify(
            pinned.addr,
            &root,
            None,
            crate::verify::VerifyKind::Check,
            None,
            10,
        ),
    )
    .await
    .unwrap();
    assert_eq!(unpinned.executor.unwrap().confirmed_node, None);
}
