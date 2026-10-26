/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

#![cfg(unix)]

#[path = "shadow_executable_mode_native/gateway.rs"]
mod gateway;

use gateway::{Gateway, Workspace, snapshot};
use prod_code_mcp::shadow::{HypothesisEdit, HypothesisSpec, parse_specs};
use std::os::unix::fs::symlink;

const BASELINE: &str = include_str!("shadow_executable_mode_native/fixtures/baseline-check.sh");
const EDITED: &str = include_str!("shadow_executable_mode_native/fixtures/edited-check.sh");
const REPLACED: &str = include_str!("shadow_executable_mode_native/fixtures/replaced-link.sh");

#[tokio::test]
async fn edited_executable_script_preserves_mode_and_runs_directly_in_shadow() {
    let ws = Workspace::new();
    ws.write("scripts/check.sh", BASELINE, 0o755);
    ws.write("scripts/obsolete.sh", "#!/bin/sh\necho obsolete\n", 0o755);
    ws.write("notes/readme.txt", "before\n", 0o644);
    ws.commit().await;
    let root = ws.root();
    let before = snapshot(&root);
    let specs = parse_specs(
        &root,
        &serde_json::json!({
            "hypotheses": [{
                "name": "edited-script",
                "edits": [
                    { "path": "scripts/check.sh", "new_text": EDITED },
                    { "path": "notes/readme.txt", "new_text": "updated-note" },
                    { "path": "notes/created.txt", "new_text": "new-ordinary-file" }
                ],
                "delete": ["scripts/obsolete.sh"]
            }]
        }),
        None,
    )
    .unwrap();
    let gateway = Gateway::new().await;
    let (control, _) = gateway
        .run(
            &root,
            &[HypothesisSpec {
                name: "original-executable-control".to_string(),
                edits: Vec::new(),
            }],
            &["./scripts/check.sh"],
        )
        .await;
    assert_eq!(control.results[0].exit_code, Some(1));
    assert_eq!(control.results[0].output, "baseline-fail\n");
    assert!(control.results[0].error.is_none() && !control.results[0].timed_out);
    let server = std::path::Path::new(&control.server_workspace_root);
    let server_before = snapshot(server);
    let (outcome, requests) = gateway.run(&root, &specs, &["./scripts/check.sh"]).await;
    let result = &outcome.results[0];
    let deltas = &requests[0].hypotheses[0].files;
    let find = |path: &str| deltas.iter().find(|d| d.relative_path == path).unwrap();
    println!(
        "flags={:?}",
        deltas
            .iter()
            .map(|d| (&d.relative_path, d.is_executable))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        snapshot(&root),
        before,
        "client files/modes must be unchanged"
    );
    assert_eq!(
        snapshot(server),
        server_before,
        "production server workspace must be restored"
    );
    assert!(matches!(
        outcome.mode.as_str(),
        "overlay" | "overlay-ram" | "in-place"
    ));
    assert_eq!(
        result.exit_code,
        Some(0),
        "error={:?} output={:?}",
        result.error,
        result.output
    );
    assert!(result.error.is_none() && !result.timed_out);
    assert_eq!(
        result.output.trim(),
        "shadow-script-ok:updated-note:new-ordinary-file"
    );
    assert_eq!(outcome.winner, Some(0));
    assert!(find("scripts/check.sh").is_executable);
    assert!(!find("notes/readme.txt").is_executable);
    assert!(!find("notes/created.txt").is_executable);
    assert!(find("scripts/obsolete.sh").content.is_none());
    assert!(!find("scripts/obsolete.sh").is_executable);
}

#[tokio::test]
async fn symlink_and_outside_paths_do_not_inherit_unrelated_executable_modes() {
    let outside = Workspace::new();
    let outside_exec = outside.write("outside_exec.sh", "#!/bin/sh\necho outside\n", 0o755);
    outside.write("sub/nested.sh", "#!/bin/sh\necho nested\n", 0o755);
    let outside_before = snapshot(&outside.root());
    let ws = Workspace::new();
    ws.write("scripts/local.sh", "#!/bin/sh\necho local\n", 0o755);
    symlink(&outside_exec, ws.root().join("scripts/external_link.sh")).unwrap();
    symlink("local.sh", ws.root().join("scripts/internal_link.sh")).unwrap();
    symlink(outside.root().join("sub"), ws.root().join("linked_dir")).unwrap();
    ws.commit().await;
    let root = ws.root();
    let before = snapshot(&root);
    let gateway = Gateway::new().await;
    let spec = |name: &str, paths: &[&str]| HypothesisSpec {
        name: name.to_string(),
        edits: paths
            .iter()
            .map(|path| HypothesisEdit {
                relative_path: path.to_string(),
                text: Some(REPLACED.to_string()),
            })
            .collect(),
    };
    // Real sync deliberately materializes contained links and omits outside links. Seed
    // first, then install owned server links so production check_files sees the hazards too.
    let (seed, _) = gateway
        .run(&root, &[spec("seed", &[])], &["sh", "-c", "printf seed-ok"])
        .await;
    assert_eq!(seed.results[0].exit_code, Some(0));
    assert_eq!(seed.results[0].output, "seed-ok");
    let server = std::path::Path::new(&seed.server_workspace_root);
    for (link, target) in [
        ("scripts/external_link.sh", outside_exec.clone()),
        (
            "scripts/internal_link.sh",
            std::path::PathBuf::from("local.sh"),
        ),
        ("linked_dir", outside.root().join("sub")),
    ] {
        let path = server.join(link);
        if std::fs::symlink_metadata(&path).is_ok() {
            std::fs::remove_file(&path).unwrap();
        }
        symlink(target, path).unwrap();
    }
    let server_before = snapshot(server);
    assert_eq!(root.parent(), outside.root().parent());
    let traversal = format!(
        "../{}/outside_exec.sh",
        outside.root().file_name().unwrap().to_str().unwrap()
    );
    let specs = [
        spec(
            "replace-links",
            &["scripts/external_link.sh", "scripts/internal_link.sh"],
        ),
        spec("symlink-parent", &["linked_dir/nested.sh"]),
        spec("traversal", &[&traversal]),
        spec("absolute", &[outside_exec.to_str().unwrap()]),
    ];
    // The same listener address keeps the real sync watermark valid across seed and test.
    let (outcome, requests) = gateway.run(&root, &specs, &["sh", "-c",
        "set -eu; for p in scripts/external_link.sh scripts/internal_link.sh; do test ! -L \"$p\"; test ! -x \"$p\"; sh \"$p\"; done"
    ]).await;
    assert_eq!(snapshot(&root), before, "client files/modes/links changed");
    assert_eq!(
        snapshot(&outside.root()),
        outside_before,
        "outside files/modes changed"
    );
    assert_eq!(
        snapshot(server),
        server_before,
        "server files/modes/links changed"
    );
    for delta in requests[0].hypotheses.iter().flat_map(|h| &h.files) {
        assert!(
            !delta.is_executable,
            "unsafe metadata inheritance for {:?}",
            delta.relative_path
        );
    }
    assert_eq!(
        outcome
            .results
            .iter()
            .map(|result| result.name.as_str())
            .collect::<Vec<_>>(),
        ["replace-links", "symlink-parent", "traversal", "absolute"],
        "every submitted hypothesis must return a result in order"
    );
    let replaced = &outcome.results[0];
    assert_eq!(replaced.exit_code, Some(0), "{replaced:?}");
    assert!(replaced.error.is_none() && !replaced.timed_out);
    assert_eq!(replaced.output, "replaced-link\nreplaced-link\n");
    for (result, reason) in
        outcome.results[1..]
            .iter()
            .zip(["symbolic link", "invalid path", "invalid path"])
    {
        assert!(
            result.exit_code.is_none() && result.output.is_empty() && !result.timed_out,
            "{result:?}"
        );
        assert!(
            result.error.as_deref().unwrap_or_default().contains(reason),
            "{result:?}"
        );
    }
}
