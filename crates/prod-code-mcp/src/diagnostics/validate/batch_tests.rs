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
use std::path::PathBuf;

#[tokio::test]
async fn runtime_paths_2_batch_mixed_engines_preserve_edit_and_caller_order() {
    let files = [
        ("a.rs", "fn a() {}"),
        ("a.json", "{}"),
        ("a.py", "x = 1"),
        ("a.md", "# A\n"),
        ("a.xml", "<root/>"),
        ("b.rs", "fn b() {}"),
        ("b.json", "[]"),
        ("b.py", "y = 2"),
        ("b.md", "# B\n"),
        ("b.xml", "<root/>"),
    ];
    let ws = Workspace::new(&files);
    let gateway = ScriptedGateway::start(|method, _| {
        if method == "textDocument/diagnostic" {
            answers::no_diagnostics()
        } else {
            serde_json::json!([])
        }
    })
    .await;
    let edits: Vec<_> = files[..5]
        .iter()
        .map(|(p, t)| (PathBuf::from(p), t.to_string()))
        .collect();
    let also: Vec<_> = files[5..].iter().map(|(p, _)| PathBuf::from(p)).collect();
    let progress = ValidationProgress::new(10);
    let reports = validate_texts_interactive_with_progress(
        gateway.addr(),
        &ws.root(),
        &edits,
        &also,
        progress.clone(),
    )
    .await
    .unwrap();
    assert_eq!(
        reports
            .iter()
            .map(|r| (r.file.as_str(), r.errors, r.warnings, r.items.len()))
            .collect::<Vec<_>>(),
        files.iter().map(|(p, _)| (*p, 0, 0, 0)).collect::<Vec<_>>()
    );
    assert_eq!(progress.snapshot().1, 10);
    for (path, original) in files {
        assert_eq!(std::fs::read_to_string(ws.path(path)).unwrap(), original);
    }
}

#[tokio::test]
async fn runtime_paths_2_batch_parser_only_empty_and_refusals() {
    let ws = Workspace::new(&[("caller.json", "[]")]);
    let remote = "127.0.0.1:1".parse().unwrap();
    assert_eq!(
        validate_texts(remote, &ws.root(), &[], &[])
            .await
            .unwrap()
            .len(),
        0
    );
    let edits = [(PathBuf::from("new.json"), "{\"x\":1}".into())];
    let reports = validate_texts_interactive(remote, &ws.root(), &edits, &[ws.path("caller.json")])
        .await
        .unwrap();
    assert_eq!(
        reports
            .iter()
            .map(|r| (r.file.as_str(), r.errors, r.items.len()))
            .collect::<Vec<_>>(),
        [("new.json", 0, 0), ("caller.json", 0, 0)]
    );
    let missing = ws.path("missing.json");
    let error = validate_texts(remote, &ws.root(), &edits, std::slice::from_ref(&missing))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with(&format!(
            "cannot read {}, which the change must be checked against; nothing was validated:",
            missing.display()
        )),
        "{error}"
    );
    let unsupported = validate_texts(
        remote,
        &ws.root(),
        &[("photo.png".into(), String::new())],
        &[],
    )
    .await
    .unwrap_err();
    assert!(
        unsupported.to_string().contains("photo.png"),
        "{unsupported}"
    );
}
