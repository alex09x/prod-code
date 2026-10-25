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
async fn ignored_outputs_update_and_preserve_local_mutations_in_both_runners() {
    let mut failures = vec![];
    for poly in [false, true] {
        let ws = Workspace::new(&[(
            ".gitignore",
            "report.txt\ndeleted.txt\nrenamed.txt\nremove.txt\nsame.txt\nnew.txt\ngenerated/\n",
        )]);
        for (path, content) in [
            ("report.txt", "earlier report"),
            ("deleted.txt", "local deletion"),
            ("renamed.txt", "rename bytes"),
            ("remove.txt", "remote deletion"),
            ("same.txt", "unchanged"),
            ("generated/nested.txt", "earlier nested report"),
        ] {
            ws.write(path, content);
        }
        let unchanged_time = std::fs::metadata(ws.path("same.txt"))
            .unwrap()
            .modified()
            .unwrap();
        let g = gateway(
            vec![
                delta("report.txt", Some("regenerated report")),
                delta("deleted.txt", Some("remote deleted-path report")),
                delta("renamed.txt", Some("remote renamed-path report")),
                delta("remove.txt", None),
                delta("same.txt", Some("unchanged")),
                delta("generated/nested.txt", Some("regenerated nested report")),
                delta("new.txt", Some("new report")),
            ],
            other_os(),
        )
        .await;
        let root = ws.root();
        let client = run(g.address, &root, poly, &["fixture"]);
        let edits = async {
            g.arrived.await.unwrap();
            std::fs::remove_file(ws.path("deleted.txt")).unwrap();
            std::fs::rename(ws.path("renamed.txt"), ws.path("destination.txt")).unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, edits);
        g.task.await.unwrap();
        let deleted_exists = ws.path("deleted.txt").exists();
        let renamed_exists = ws.path("renamed.txt").exists();
        let removed_exists = ws.path("remove.txt").exists();
        let unchanged = ws.read("same.txt") == "unchanged"
            && std::fs::metadata(ws.path("same.txt"))
                .unwrap()
                .modified()
                .unwrap()
                == unchanged_time
            && !out.pulled_files.contains(&"same.txt".into());
        println!(
            "ignored poly={poly}: report={:?} nested={:?} deleted_exists={deleted_exists} renamed_exists={renamed_exists} removed_exists={removed_exists} destination={:?} new={:?} unchanged={unchanged} kept={:?} pulled={:?}",
            ws.read("report.txt"),
            ws.read("generated/nested.txt"),
            ws.read("destination.txt"),
            ws.read("new.txt"),
            out.kept_files,
            out.pulled_files
        );
        if ws.read("report.txt") != "regenerated report"
            || ws.read("generated/nested.txt") != "regenerated nested report"
            || deleted_exists
            || renamed_exists
            || removed_exists
            || ws.read("destination.txt") != "rename bytes"
            || ws.read("new.txt") != "new report"
            || !unchanged
            || out.kept_files != ["deleted.txt", "renamed.txt"]
            || out.pulled_files
                != [
                    "report.txt",
                    "remove.txt",
                    "generated/nested.txt",
                    "new.txt",
                ]
        {
            failures.push(poly);
        }
    }
    assert!(
        failures.is_empty(),
        "ignored-output runners failed: {failures:?}"
    );
}

#[tokio::test]
async fn excluded_node_caches_do_not_exhaust_writeback_snapshot() {
    let ws = Workspace::new(&[(".gitignore", ".cache/\n.venv/\n.next/\ngenerated/\n")]);
    // More than the supplementary file limit, all outside gateway writeback traversal.
    for i in 0..10_001 {
        ws.write(&format!(".cache/entries/{i}.txt"), "cached bytes");
    }
    ws.write(".venv/lib/installed.txt", "installed bytes");
    ws.write(".next/cache/page.txt", "page bytes");
    let mut failures = vec![];
    for poly in [false, true] {
        ws.write("generated/report.txt", "earlier report");
        ws.write("generated/deleted.txt", "local deletion");
        let g = gateway(
            vec![
                delta("generated/report.txt", Some("regenerated report")),
                delta("generated/deleted.txt", Some("remote resurrection")),
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
                // A preflight failure must not leave the fixture waiting for a connection.
                g.task.abort();
                let _ = g.task.await;
                println!("cache poly={poly}: reached_gateway=false failed={}", result.is_err());
                failures.push(poly);
                continue;
            }
            arrived = g.arrived => arrived.unwrap(),
        }
        std::fs::remove_file(ws.path("generated/deleted.txt")).unwrap();
        g.release.send(()).unwrap();
        let out = client.await.unwrap();
        g.task.await.unwrap();
        let cache_unchanged = ws.read(".cache/entries/10000.txt") == "cached bytes"
            && ws.read(".venv/lib/installed.txt") == "installed bytes"
            && ws.read(".next/cache/page.txt") == "page bytes";
        println!(
            "cache poly={poly}: reached_gateway=true cache_unchanged={cache_unchanged} report={:?} deleted_exists={} kept={:?} pulled={:?}",
            ws.read("generated/report.txt"),
            ws.path("generated/deleted.txt").exists(),
            out.kept_files,
            out.pulled_files
        );
        assert!(cache_unchanged);
        assert_eq!(ws.read("generated/report.txt"), "regenerated report");
        assert!(!ws.path("generated/deleted.txt").exists());
        assert_eq!(out.kept_files, ["generated/deleted.txt"]);
        assert_eq!(out.pulled_files, ["generated/report.txt"]);
    }
    assert!(failures.is_empty(), "cache preflight failed: {failures:?}");
}
