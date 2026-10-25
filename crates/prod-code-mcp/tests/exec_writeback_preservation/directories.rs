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
async fn ignored_directories_have_initial_identity_in_both_runners() {
    let mut failures = vec![];
    for poly in [false, true] {
        for dir in ["artifacts", "results"] {
            let ws = Workspace::new(&[(".gitignore", "artifacts/\nresults/\n")]);
            for (name, bytes) in [
                ("report.txt", "earlier report"),
                ("remove.txt", "remote deletion"),
                ("deleted.txt", "local deletion"),
                ("renamed.txt", "rename bytes"),
            ] {
                ws.write(&format!("{dir}/{name}"), bytes);
            }
            let report = format!("{dir}/report.txt");
            let remove = format!("{dir}/remove.txt");
            let deleted = format!("{dir}/deleted.txt");
            let renamed = format!("{dir}/renamed.txt");
            let destination = format!("{dir}/destination.txt");
            let new = format!("{dir}/new.txt");
            let g = gateway(
                vec![
                    delta(&report, Some("regenerated report")),
                    delta(&remove, None),
                    delta(&deleted, Some("remote resurrection")),
                    delta(&renamed, Some("remote resurrection")),
                    delta(&new, Some("genuine new output")),
                ],
                other_os(),
            )
            .await;
            let root = ws.root();
            let client = run(g.address, &root, poly, &["fixture"]);
            let edits = async {
                g.arrived.await.unwrap();
                std::fs::remove_file(ws.path(&deleted)).unwrap();
                std::fs::rename(ws.path(&renamed), ws.path(&destination)).unwrap();
                g.release.send(()).unwrap();
            };
            let (out, ()) = tokio::join!(client, edits);
            g.task.await.unwrap();
            let bytes = |path: &str| std::fs::read(ws.path(path)).ok();
            println!(
                "directories poly={poly} dir={dir}: report={:?} remove={:?} deleted={:?} renamed={:?} destination={:?} new={:?} kept={:?} pulled={:?}",
                bytes(&report),
                bytes(&remove),
                bytes(&deleted),
                bytes(&renamed),
                bytes(&destination),
                bytes(&new),
                out.kept_files,
                out.pulled_files
            );
            if ws.read(&report) != "regenerated report"
                || ws.path(&remove).exists()
                || ws.path(&deleted).exists()
                || ws.path(&renamed).exists()
                || ws.read(&destination) != "rename bytes"
                || ws.read(&new) != "genuine new output"
                || out.kept_files != [deleted, renamed]
                || out.pulled_files != [report, remove, new]
            {
                failures.push((poly, dir));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "directory identity failures: {failures:?}"
    );
}

#[tokio::test]
async fn large_old_outputs_can_shrink_or_be_deleted_in_both_runners() {
    let mut failures = vec![];
    for poly in [false, true] {
        // Above the gateway content-return limit and the separate upload limit.
        for (dir, size) in [
            ("artifacts", 5 * 1024 * 1024 + 1),
            ("results", 10 * 1024 * 1024 + 1),
        ] {
            let ws = Workspace::new(&[(".gitignore", "artifacts/\nresults/\n")]);
            let report = format!("{dir}/large.txt");
            let remove = format!("{dir}/remove.txt");
            let deleted = format!("{dir}/deleted.txt");
            for path in [&report, &remove, &deleted] {
                ws.write(path, "");
                std::fs::File::options()
                    .write(true)
                    .open(ws.path(path))
                    .unwrap()
                    .set_len(size)
                    .unwrap();
            }
            let g = gateway(
                vec![
                    delta(&report, Some("small replacement")),
                    delta(&remove, None),
                    delta(&deleted, Some("remote resurrection")),
                ],
                other_os(),
            )
            .await;
            let root = ws.root();
            let client = run(g.address, &root, poly, &["fixture"]);
            let edits = async {
                g.arrived.await.unwrap();
                std::fs::remove_file(ws.path(&deleted)).unwrap();
                g.release.send(()).unwrap();
            };
            let (out, ()) = tokio::join!(client, edits);
            g.task.await.unwrap();
            println!(
                "large poly={poly} dir={dir} initial=zero_bytes({size}): report={:?} remove_exists={} deleted_exists={} kept={:?} pulled={:?}",
                std::fs::read(ws.path(&report)).ok(),
                ws.path(&remove).exists(),
                ws.path(&deleted).exists(),
                out.kept_files,
                out.pulled_files
            );
            if ws.read(&report) != "small replacement"
                || ws.path(&remove).exists()
                || ws.path(&deleted).exists()
                || out.kept_files != [deleted]
                || out.pulled_files != [report, remove]
            {
                failures.push((poly, dir));
            }
        }
    }
    assert!(failures.is_empty(), "large-output failures: {failures:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn unknown_link_outputs_are_not_mistaken_for_new_paths() {
    for poly in [false, true] {
        let ws = Workspace::new(&[(".gitignore", "artifacts/\nresults/\n")]);
        ws.write("results/report.txt", "original target");
        std::fs::create_dir(ws.path("artifacts")).unwrap();
        std::os::unix::fs::symlink("../results", ws.path("artifacts/linked")).unwrap();
        std::os::unix::fs::symlink("missing", ws.path("artifacts/unknown.txt")).unwrap();
        let g = gateway(
            vec![
                delta("artifacts/linked/report.txt", Some("remote resurrection")),
                delta("artifacts/unknown.txt", Some("remote resurrection")),
                delta("artifacts/new.txt", Some("genuine new output")),
            ],
            other_os(),
        )
        .await;
        let root = ws.root();
        let client = run(g.address, &root, poly, &["fixture"]);
        let edits = async {
            g.arrived.await.unwrap();
            std::fs::remove_file(ws.path("artifacts/linked")).unwrap();
            std::fs::remove_file(ws.path("artifacts/unknown.txt")).unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, edits);
        g.task.await.unwrap();
        println!(
            "unknown poly={poly}: kept={:?} pulled={:?}",
            out.kept_files, out.pulled_files
        );
        assert!(!ws.path("artifacts/linked").exists());
        assert!(!ws.path("artifacts/unknown.txt").exists());
        assert_eq!(ws.read("results/report.txt"), "original target");
        assert_eq!(ws.read("artifacts/new.txt"), "genuine new output");
        assert_eq!(
            out.kept_files,
            ["artifacts/linked/report.txt", "artifacts/unknown.txt"]
        );
        assert_eq!(out.pulled_files, ["artifacts/new.txt"]);
    }
}

#[tokio::test]
async fn oversized_output_stays_protected_without_blocking_execution() {
    for poly in [false, true] {
        let ws = Workspace::new(&[(".gitignore", "artifacts/\n")]);
        ws.write("artifacts/oversized.txt", "");
        std::fs::File::options()
            .write(true)
            .open(ws.path("artifacts/oversized.txt"))
            .unwrap()
            .set_len(256 * 1024 * 1024 + 1)
            .unwrap();
        let g = gateway(
            vec![
                delta("artifacts/oversized.txt", Some("unverified replacement")),
                delta("artifacts/new.txt", Some("verified new output")),
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
                panic!("oversized output prevented execution, poly={poly}, panic={}", result.is_err());
            }
            arrived = g.arrived => arrived.unwrap(),
        }
        g.release.send(()).unwrap();
        let outcome = client.await.unwrap();
        g.task.await.unwrap();
        assert_eq!(outcome.kept_files, ["artifacts/oversized.txt"]);
        assert_eq!(outcome.pulled_files, ["artifacts/new.txt"]);
        assert_eq!(ws.read("artifacts/new.txt"), "verified new output");
        assert_eq!(
            std::fs::metadata(ws.path("artifacts/oversized.txt"))
                .unwrap()
                .len(),
            256 * 1024 * 1024 + 1
        );
    }
}
