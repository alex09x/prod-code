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
async fn local_deletion_rename_and_same_mtime_edit_survive_both_runners() {
    let mut failures = vec![];
    for poly in [false, true] {
        let ws = Workspace::new(&[
            ("deleted.rs", "original"),
            ("renamed.rs", "rename bytes"),
            ("edited.rs", "before"),
            ("update.rs", "before"),
            ("remove.rs", "remove"),
            ("same.rs", "unchanged"),
        ]);
        let g = gateway(
            vec![
                delta("deleted.rs", Some("remote")),
                delta("renamed.rs", Some("remote")),
                delta("edited.rs", Some("remote")),
                delta("update.rs", Some("updated")),
                delta("remove.rs", None),
                delta("same.rs", Some("unchanged")),
                delta("new.rs", Some("generated")),
            ],
            other_os(),
        )
        .await;
        let root = ws.root();
        let unchanged_time = std::fs::metadata(ws.path("same.rs"))
            .unwrap()
            .modified()
            .unwrap();
        let client = run(g.address, &root, poly, &["fixture"]);
        let edits = async {
            g.arrived.await.unwrap();
            std::fs::remove_file(ws.path("deleted.rs")).unwrap();
            std::fs::rename(ws.path("renamed.rs"), ws.path("destination.rs")).unwrap();
            let time = std::fs::metadata(ws.path("edited.rs"))
                .unwrap()
                .modified()
                .unwrap();
            ws.write("edited.rs", "locally");
            std::fs::File::options()
                .write(true)
                .open(ws.path("edited.rs"))
                .unwrap()
                .set_modified(time)
                .unwrap();
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, edits);
        g.task.await.unwrap();
        let preserved = !ws.path("deleted.rs").exists()
            && !ws.path("renamed.rs").exists()
            && ws.read("destination.rs") == "rename bytes"
            && ws.read("edited.rs") == "locally";
        println!(
            "preservation poly={poly}: preserved={preserved} kept={:?} pulled={:?}",
            out.kept_files, out.pulled_files
        );
        println!(
            "preserved bytes poly={poly}: destination={:?} edited={:?} unchanged={:?}",
            ws.read("destination.rs"),
            ws.read("edited.rs"),
            ws.read("same.rs")
        );
        if !preserved || out.kept_files != ["deleted.rs", "renamed.rs", "edited.rs"] {
            failures.push(poly);
        }
        assert_eq!(ws.read("update.rs"), "updated");
        assert_eq!(ws.read("new.rs"), "generated");
        assert_eq!(ws.read("same.rs"), "unchanged");
        assert_eq!(
            std::fs::metadata(ws.path("same.rs"))
                .unwrap()
                .modified()
                .unwrap(),
            unchanged_time
        );
        assert!(!out.pulled_files.contains(&"same.rs".into()));
        assert!(!ws.path("remove.rs").exists());
    }
    assert!(
        failures.is_empty(),
        "local mutations lost in runners: {failures:?}"
    );
}
