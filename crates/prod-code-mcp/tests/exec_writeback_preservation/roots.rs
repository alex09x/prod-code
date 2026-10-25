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
use prod_code_testkit::Workspace;

#[tokio::test]
async fn symlink_root_preserves_writeback_identity_in_both_runners() {
    let mut failures = vec![];
    for poly in [false, true] {
        let ws = Workspace::new(&[(".gitignore", "artifacts/\nresults/\n")]);
        let prefixes = ["src", "artifacts", "results"];
        for dir in prefixes {
            for (name, bytes) in [
                ("report.rs", "earlier report"),
                ("remove.rs", "remote deletion"),
                ("deleted.rs", "local deletion"),
                ("renamed.rs", "rename bytes"),
            ] {
                ws.write(&format!("{dir}/{name}"), bytes);
            }
        }
        // Keep this alias outside the workspace and pass it without canonicalization.
        // Workspace::root() itself canonicalizes and would hide this traversal case.
        let alias = tempfile::tempdir().unwrap();
        let root = alias.path().join("workspace");
        std::os::unix::fs::symlink(ws.root(), &root).unwrap();
        assert!(std::fs::symlink_metadata(&root).unwrap().is_symlink());
        let mut returned = vec![];
        let mut expected_kept = vec![];
        let mut expected_pulled = vec![];
        let bytes = |path: &str| std::fs::read(ws.path(path)).ok();
        for dir in prefixes {
            let report = format!("{dir}/report.rs");
            let remove = format!("{dir}/remove.rs");
            let deleted = format!("{dir}/deleted.rs");
            let renamed = format!("{dir}/renamed.rs");
            let new = format!("{dir}/new.rs");
            println!(
                "symlink initial poly={poly} dir={dir}: report={:?} remove={:?} deleted={:?} renamed={:?} new={:?}",
                bytes(&report),
                bytes(&remove),
                bytes(&deleted),
                bytes(&renamed),
                bytes(&new)
            );
            returned.extend([
                delta(&report, Some("regenerated report")),
                delta(&remove, None),
                delta(&deleted, Some("remote resurrection")),
                delta(&renamed, Some("remote resurrection")),
                delta(&new, Some("genuine new output")),
            ]);
            expected_kept.extend([deleted, renamed]);
            expected_pulled.extend([report, remove, new]);
        }
        let g = gateway(returned, other_os()).await;
        let client = run(g.address, &root, poly, &["fixture"]);
        let edits = async {
            g.arrived.await.unwrap();
            for dir in prefixes {
                std::fs::remove_file(ws.path(&format!("{dir}/deleted.rs"))).unwrap();
                std::fs::rename(
                    ws.path(&format!("{dir}/renamed.rs")),
                    ws.path(&format!("{dir}/destination.rs")),
                )
                .unwrap();
            }
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, edits);
        g.task.await.unwrap();
        for dir in prefixes {
            let report = bytes(&format!("{dir}/report.rs"));
            let remove = bytes(&format!("{dir}/remove.rs"));
            let deleted = bytes(&format!("{dir}/deleted.rs"));
            let renamed = bytes(&format!("{dir}/renamed.rs"));
            let destination = bytes(&format!("{dir}/destination.rs"));
            let new = bytes(&format!("{dir}/new.rs"));
            println!(
                "symlink final poly={poly} dir={dir}: report={report:?} remove={remove:?} deleted={deleted:?} renamed={renamed:?} destination={destination:?} new={new:?} kept={:?} pulled={:?}",
                out.kept_files, out.pulled_files
            );
            if report.as_deref() != Some(b"regenerated report".as_slice())
                || remove.is_some()
                || deleted.is_some()
                || renamed.is_some()
                || destination.as_deref() != Some(b"rename bytes".as_slice())
                || new.as_deref() != Some(b"genuine new output".as_slice())
                || out.kept_files != expected_kept
                || out.pulled_files != expected_pulled
            {
                failures.push((poly, dir));
            }
        }
    }
    assert!(failures.is_empty(), "symlink-root failures: {failures:?}");
}
