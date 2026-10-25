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
use prod_code_testkit::{ScriptedGateway, Workspace, answers};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn runtime_paths_2_single_disk_retries_once_and_missing_disk_is_absent() {
    let ws = Workspace::new(&[("a.rs", "fn a() {}\r\n")]);
    for failures in [1, 2] {
        let queries = Arc::new(AtomicUsize::new(0));
        let gateway = ScriptedGateway::start({
            let queries = queries.clone();
            move |method, _| {
                if method == "textDocument/diagnostic" {
                    if queries.fetch_add(1, Ordering::SeqCst) < failures {
                        answers::failure("snapshot unavailable")
                    } else {
                        answers::error_at(1, 4, "E0425", "cannot find value `a`")
                    }
                } else {
                    serde_json::json!([])
                }
            }
        })
        .await;
        let mut session = LspSession::open(gateway.addr(), &ws.root(), Some(Path::new("a.rs")))
            .await
            .unwrap();
        assert!(
            on_disk(
                &mut session,
                &ws.root(),
                Path::new("absent.rs"),
                "absent.rs"
            )
            .await
            .is_none()
        );
        let report = on_disk(&mut session, &ws.root(), &ws.path("a.rs"), "a.rs").await;
        assert_eq!(queries.load(Ordering::SeqCst), 2);
        if failures == 1 {
            let (report, text) = report.unwrap();
            assert_eq!(text, "fn a() {}\r\n");
            assert_eq!(
                (
                    report.file.as_str(),
                    report.errors,
                    report.items[0].line,
                    report.items[0].col
                ),
                ("a.rs", 1, 1, 4)
            );
            assert_eq!(report.items[0].message, "cannot find value `a`");
        } else {
            assert!(report.is_none());
        }
        session.close().await;
    }
}

#[tokio::test]
async fn runtime_paths_2_single_parser_disk_and_proposal_never_write() {
    let ws = Workspace::new(&[("a.json", "{}")]);
    let remote = "127.0.0.1:1".parse().unwrap();
    for file in [PathBuf::from("a.json"), ws.path("a.json")] {
        let report = diagnostics(remote, &ws.root(), &file).await.unwrap();
        assert_eq!(
            (report.file.as_str(), report.errors, report.items.len()),
            ("a.json", 0, 0)
        );
        let report = validate_text(remote, &ws.root(), &file, "[").await.unwrap();
        assert_eq!(
            (report.errors, report.items[0].line, report.items[0].col),
            (1, 1, 1)
        );
        assert_eq!(
            report.items[0].message,
            "JSON syntax error: EOF while parsing a list at line 1 column 1"
        );
    }
    assert_eq!(ws.read("a.json"), "{}");
    assert_eq!(
        diagnostics(remote, &ws.root(), Path::new("absent.json"))
            .await
            .unwrap_err()
            .to_string(),
        "cannot read absent.json"
    );
    for proposal in [false, true] {
        let err = if proposal {
            validate_text(remote, &ws.root(), Path::new("a.png"), "").await
        } else {
            diagnostics(remote, &ws.root(), Path::new("a.png")).await
        }
        .unwrap_err();
        assert!(err.to_string().starts_with(
            "semantic diagnostics are not supported for a.png; no validation was performed."
        ));
    }
}

#[tokio::test]
async fn runtime_paths_2_single_semantic_baseline_new_file_and_disk_diagnostics() {
    let ws = Workspace::new(&[("a.rs", "fn old() {}\n")]);
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" {
            answers::error_at(1, 1, "E0425", "cannot find value `missing` in this scope")
        } else {
            serde_json::json!([])
        }
    })
    .await;
    for file in [PathBuf::from("a.rs"), ws.path("a.rs")] {
        let report = diagnostics(gateway.addr(), &ws.root(), &file)
            .await
            .unwrap();
        assert_eq!(
            (
                report.file.as_str(),
                report.errors,
                report.items[0].code.as_deref()
            ),
            ("a.rs", 1, Some("E0425"))
        );
        let report = validate_text(gateway.addr(), &ws.root(), &file, "fn old() {}\n")
            .await
            .unwrap();
        assert_eq!(
            (report.errors, report.preexisting.len(), report.items.len()),
            (0, 1, 0)
        );
    }
    let report = validate_text(gateway.addr(), &ws.root(), Path::new("new.rs"), "missing\n")
        .await
        .unwrap();
    assert_eq!(
        (
            report.file.as_str(),
            report.errors,
            report.preexisting.len()
        ),
        ("new.rs", 1, 0)
    );
    assert_eq!(
        report.items[0].message,
        "cannot find value `missing` in this scope"
    );
    assert_eq!(ws.read("a.rs"), "fn old() {}\n");
    assert!(!ws.path("new.rs").exists());
}

use std::path::PathBuf;
