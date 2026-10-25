/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use prod_code_protocol::{ShadowHypothesisResult, ShadowRunResponse};

#[tokio::test]
async fn unsuccessful_protocol_outcomes_never_become_clean_compiler_success() {
    // Scripted wire outcomes only: no real compiler execution in this fixture.
    for (exit, error, timed_out, output, missing, expected) in [
        (
            Some(9),
            None,
            false,
            "opaque nonzero output",
            false,
            "exit=9",
        ),
        (
            None,
            None,
            false,
            "opaque missing exit",
            false,
            "exit=unknown",
        ),
        (
            Some(0),
            Some("launch failed"),
            false,
            "launch output",
            false,
            "launch failed",
        ),
        (
            Some(0),
            None,
            true,
            "partial timeout output",
            false,
            "timed_out=true",
        ),
        (Some(0), None, false, "", true, "missing-result"),
        (
            Some(0),
            None,
            false,
            "error[E0583]: file not found for module `gone`\n",
            false,
            "E0583",
        ),
    ] {
        let response = ShadowRunResponse {
            server_workspace_root: String::new(),
            mode: "scripted-protocol-only".into(),
            error: None,
            results: if missing {
                vec![]
            } else {
                vec![ShadowHypothesisResult {
                    name: "proposed".into(),
                    exit_code: exit,
                    duration_ms: 1,
                    timed_out,
                    error: error.map(str::to_string),
                    output_len: output.len() as u64,
                    output_tail: Some(output.as_bytes().to_vec()),
                }]
            },
        };
        let workspace = Workspace::new(FILES);
        let gateway = CompilerGateway::scripted(response).await;
        let result = execute_tool(
            gateway.addr,
            &workspace.root(),
            "code_validate_edits",
            json!({"diff":DELETE_UNUSED,"compile":true}),
        )
        .await;
        let report = match result {
            Ok(r) => {
                assert!(r.is_error, "{}", text(&r));
                text(&r)
            }
            Err(e) => format!("{e:#}"),
        };
        assert!(report.contains(expected), "{report}");
        assert!(report.contains("argv=[\"cargo\",\"check\",\"--workspace\",\"--all-targets\",\"--message-format=json\"]"),"{report}");
        assert!(report.contains("cwd=\".\""), "{report}");
        assert!(report.contains("timeout_secs=900"), "{report}");
        assert!(
            !report.contains("configured compiler check passed"),
            "{report}"
        );
        if expected == "exit=9" || expected == "exit=unknown" || timed_out || error.is_some() {
            assert!(report.contains(output), "{report}");
        }
        assert_eq!(gateway.requests.lock().unwrap().len(), 1);
        assert_unchanged(&workspace);
    }
}
