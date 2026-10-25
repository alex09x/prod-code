/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! What `run_shadow` refuses, defaults, orders and records.

use super::*;

async fn run_once(
    state: &crate::ServerState,
    req: ShadowRunRequest,
) -> (Result<()>, ShadowRunResponse) {
    let (mut client, mut server) = connection();
    let run = run_shadow(state, &mut server, req);
    let receive = async {
        match client.next().await.unwrap().unwrap() {
            WireMessage::ShadowRunResponse(response) => response,
            other => panic!("expected shadow response, got {other:?}"),
        }
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::join!(run, receive)
    })
    .await
    .unwrap()
}

fn hypothesis(name: &str) -> ShadowHypothesis {
    ShadowHypothesis {
        name: name.into(),
        files: vec![],
    }
}

#[tokio::test]
async fn shadow_refuses_a_request_it_cannot_run_and_says_exactly_why() {
    let (_temp, state, workspace) = storage();
    let absent = state.storage_root.join("absent");
    type Change = fn(&mut ShadowRunRequest);
    let cases: [(&str, Change, String, &std::path::Path); 6] = [
        (
            "unsynced workspace",
            |req| req.base_workspace_name = Some("absent".into()),
            format!(
                "workspace {} is not synced to this gateway",
                absent.display()
            ),
            &absent,
        ),
        (
            "empty command",
            |req| req.command.clear(),
            "empty command".into(),
            &workspace,
        ),
        (
            "no hypotheses",
            |req| req.hypotheses.clear(),
            "no hypotheses".into(),
            &workspace,
        ),
        (
            "blank name",
            |req| req.hypotheses.push(hypothesis("  ")),
            "hypothesis name \"  \" is empty or repeated".into(),
            &workspace,
        ),
        (
            "empty name",
            |req| req.hypotheses[0].name.clear(),
            "hypothesis name \"\" is empty or repeated".into(),
            &workspace,
        ),
        (
            "repeated name",
            |req| req.hypotheses.push(hypothesis("proposal")),
            "hypothesis name \"proposal\" is empty or repeated".into(),
            &workspace,
        ),
    ];
    for (name, change, error, root) in cases {
        let mut req = request();
        change(&mut req);
        let (result, response) = run_once(&state, req).await;
        result.unwrap();
        assert_eq!(response.error.as_deref(), Some(error.as_str()), "{name}");
        assert_eq!(response.mode, "", "{name}");
        assert!(response.results.is_empty(), "{name}");
        assert_eq!(
            response.server_workspace_root,
            root.to_string_lossy(),
            "{name}"
        );
    }
    // Two different names are accepted.
    let mut req = request();
    req.hypotheses.push(hypothesis("second"));
    let (result, response) = run_once(&state, req).await;
    result.unwrap();
    assert_eq!(response.error, None);
    assert_eq!(response.results.len(), 2);
}

#[tokio::test]
async fn shadow_applies_defaults_orders_results_and_records_one_event_per_hypothesis() {
    let (_temp, state, _workspace) = storage();
    let mut req = request();
    req.timeout_secs = 0;
    req.tail_bytes = 0;
    req.parallel = 0;
    req.client_agent = None;
    req.client_host = None;
    req.command = vec![
        "sh".into(),
        "-c".into(),
        "printf out; [ \"$FAIL\" != 1 ] || exit 3".into(),
    ];
    req.env = vec![];
    req.hypotheses = vec![hypothesis("alpha"), hypothesis("beta")];
    let (result, response) = run_once(&state, req.clone()).await;
    result.unwrap();
    assert_eq!(response.error, None);
    assert!(["in-place", "overlay", "overlay-ram"].contains(&response.mode.as_str()));
    let names: Vec<_> = response.results.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"]);
    for result in &response.results {
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.output_len, 3);
        assert_eq!(result.output_tail.as_deref(), Some(b"out".as_slice()));
    }

    req.env = vec![("FAIL".into(), "1".into())];
    req.client_agent = Some("test-agent".into());
    req.client_host = Some("test-host".into());
    req.hypotheses.truncate(1);
    let (result, response) = run_once(&state, req).await;
    result.unwrap();
    assert_eq!(response.results[0].exit_code, Some(3));

    let mut events = Vec::new();
    state.metrics.for_each_ring_event(|event| {
        events.push((
            event.kind,
            event.agent.clone(),
            event.host.clone(),
            event.workspace.clone(),
            event.command.clone(),
            event.exit_code,
            event.ok,
            event.bytes,
        ))
    });
    let command = "sh -c printf out; [ \"$FAIL\" != 1 ] || exit 3";
    let event = |agent: &str, host: &str, name: &str, exit_code, ok| {
        (
            "shadow",
            agent.to_string(),
            host.to_string(),
            "runner-fixture".to_string(),
            format!("{command} [{name}]"),
            Some(exit_code),
            ok,
            3,
        )
    };
    assert_eq!(
        events,
        [
            event("unknown", "unknown", "alpha", 0, true),
            event("unknown", "unknown", "beta", 0, true),
            event("test-agent", "test-host", "alpha", 3, false),
        ]
    );
}

#[tokio::test]
async fn shadow_in_memory_requests_run_wherever_the_node_can_host_them() {
    let (_temp, state, _workspace) = storage();
    let mut req = request();
    req.in_memory = true;
    req.tail_bytes = 0;
    req.command = vec!["sh".into(), "-c".into(), "printf ram".into()];
    let (result, response) = run_once(&state, req).await;
    result.unwrap();
    assert_eq!(response.error, None);
    let expected: &[&str] = if crate::shadow::overlay_unavailable().is_some() {
        &["in-place"]
    } else {
        &["overlay", "overlay-ram"]
    };
    assert!(
        expected.contains(&response.mode.as_str()),
        "{}",
        response.mode
    );
    assert_eq!(
        response.results[0].output_tail.as_deref(),
        Some(b"ram".as_slice())
    );
}

#[tokio::test]
async fn shadow_cannot_start_overlays_in_a_shadow_root_it_cannot_create() {
    let (temp, mut state, _workspace) = storage();
    let blocker = temp.path().join("blocker");
    std::fs::write(&blocker, "a file").unwrap();
    state.shadow_root = blocker.join("shadows");
    let expected_error = format!("cannot create {}", state.shadow_root.display());
    if crate::shadow::overlay_unavailable().is_some() {
        // In place nothing is staged under the shadow root.
        let (result, response) = run_once(&state, request()).await;
        result.unwrap();
        assert_eq!((response.mode.as_str(), response.error), ("in-place", None));
    } else {
        let (_client, mut server) = connection();
        let error = run_shadow(&state, &mut server, request()).await;
        assert_eq!(error.unwrap_err().to_string(), expected_error);
    }
}
