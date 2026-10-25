/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::support::*;
use futures_util::FutureExt;
use prod_code_testkit::Workspace;

#[tokio::test]
async fn large_ignored_input_does_not_prevent_execution_or_resurrect_local_deletion() {
    let mut failures = Vec::new();
    for poly in [false, true] {
        let ws = Workspace::new(&[(".gitignore", "large.bin\n")]);
        let large = ws.path("large.bin");
        std::fs::File::create(&large)
            .unwrap()
            .set_len(prod_code_mcp::sync::MAX_NON_GIT_WORKSPACE_BYTES as u64 + 1)
            .unwrap();
        let g = gateway(
            vec![
                delta("large.bin", Some("remote output")),
                delta("small.txt", Some("new output")),
            ],
            other_os(),
        )
        .await;
        let root = ws.root();
        let client =
            std::panic::AssertUnwindSafe(run(g.address, &root, poly, &["fixture"])).catch_unwind();
        tokio::pin!(client);
        tokio::select! {
            result = &mut client => {
                g.task.abort();
                let _ = g.task.await;
                println!("large ignored input poly={poly}: execution_started=false panic={}", result.is_err());
                failures.push(poly);
                continue;
            }
            arrived = g.arrived => arrived.unwrap(),
        }
        std::fs::remove_file(&large).unwrap();
        g.release.send(()).unwrap();
        let outcome = client.await.unwrap();
        g.task.await.unwrap();
        assert!(!large.exists(), "unknown large input was resurrected");
        assert_eq!(ws.read("small.txt"), "new output");
        assert!(outcome.kept_files.contains(&"large.bin".into()));
    }
    assert!(
        failures.is_empty(),
        "ignored input prevented execution: {failures:?}"
    );
}

#[tokio::test]
async fn metadata_limit_preserves_known_source_updates_and_keeps_uncertain_outputs() {
    let ws = Workspace::new(&[
        (".gitignore", "artifacts/\n"),
        ("file.rs", "old source"),
        ("parent.rs", "old parent"),
    ]);
    for index in 0..=prod_code_mcp::sync::MAX_NON_GIT_WORKSPACE_FILES {
        ws.write(&format!("artifacts/{index}.txt"), "old output");
    }
    for poly in [false, true] {
        ws.write("file.rs", "old source");
        let g = gateway(
            vec![
                delta("file.rs", Some("new source")),
                delta("parent.rs", None),
                delta("parent.rs/child.txt", Some("proven new child")),
                delta("artifacts/new.txt", Some("unknown output")),
            ],
            other_os(),
        )
        .await;
        let root = ws.root();
        let client =
            std::panic::AssertUnwindSafe(run(g.address, &root, poly, &["fixture"])).catch_unwind();
        tokio::pin!(client);
        tokio::select! {
            result = &mut client => {
                g.task.abort();
                let _ = g.task.await;
                panic!("metadata limit prevented execution, poly={poly}, panic={}", result.is_err());
            }
            arrived = g.arrived => arrived.unwrap(),
        }
        g.release.send(()).unwrap();
        let outcome = client.await.unwrap();
        g.task.await.unwrap();
        assert_eq!(ws.read("file.rs"), "new source");
        assert!(!ws.path("artifacts/new.txt").exists());
        assert_eq!(ws.read("parent.rs/child.txt"), "proven new child");
        assert_eq!(
            outcome.pulled_files,
            ["file.rs", "parent.rs", "parent.rs/child.txt"]
        );
        assert_eq!(outcome.kept_files, ["artifacts/new.txt"]);
        std::fs::remove_dir_all(ws.path("parent.rs")).unwrap();
        ws.write("parent.rs", "old parent");
    }
}
