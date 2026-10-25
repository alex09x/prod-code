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
async fn file_to_directory_preserves_children_in_both_runners() {
    let mut failures = vec![];
    for poly in [false, true] {
        let ws = Workspace::new(&[(".gitignore", "artifacts/\n")]);
        ws.write("artifacts/report.txt", "old report\n");
        println!(
            "transition initial poly={poly}: parent={:?}",
            ws.read("artifacts/report.txt")
        );
        let g = gateway(
            vec![
                delta("artifacts/report.txt", None),
                delta("artifacts/report.txt/part.txt", Some("new child\n")),
            ],
            other_os(),
        )
        .await;
        let root = ws.root();
        let client = run(g.address, &root, poly, &["fixture"]);
        let barrier = async {
            g.arrived.await.unwrap();
            assert_eq!(ws.read("artifacts/report.txt"), "old report\n");
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, barrier);
        g.task.await.unwrap();
        let child = std::fs::read(ws.path("artifacts/report.txt/part.txt")).ok();
        println!(
            "transition final poly={poly}: directory={} child={child:?} exit={:?} kept={:?} pulled={:?}",
            ws.path("artifacts/report.txt").is_dir(),
            out.exit.exit_code,
            out.kept_files,
            out.pulled_files
        );
        if child.as_deref() != Some(b"new child\n".as_slice())
            || !out.kept_files.is_empty()
            || out.pulled_files != ["artifacts/report.txt", "artifacts/report.txt/part.txt"]
            || out.exit.exit_code != Some(0)
        {
            failures.push(poly);
        }
    }
    assert!(failures.is_empty(), "file-to-directory loss: {failures:?}");
}

#[tokio::test]
async fn file_to_directory_refreshes_identity_for_later_batches() {
    for poly in [false, true] {
        let ws = Workspace::new(&[(".gitignore", "artifacts/\nresults/\n")]);
        let parents = [
            "src/report.rs",
            "artifacts/report.txt",
            "results/report.txt",
        ];
        let mut first = vec![];
        let mut later = vec![];
        for parent in parents {
            ws.write(parent, "old report\n");
            first.extend([
                delta(parent, None),
                delta(&format!("{parent}/part.txt"), Some("first child\n")),
                delta(&format!("{parent}/nested/remove.txt"), Some("temporary\n")),
            ]);
            later.extend([
                delta(&format!("{parent}/part.txt"), Some("later child\n")),
                delta(&format!("{parent}/nested/remove.txt"), None),
                delta(&format!("{parent}/nested/new.txt"), Some("later new\n")),
            ]);
        }
        let g = gateway_batches(vec![first, later], other_os()).await;
        let root = ws.root();
        let client = run(g.address, &root, poly, &["fixture"]);
        let barrier = async {
            g.arrived.await.unwrap();
            for parent in parents {
                assert_eq!(ws.read(parent), "old report\n");
            }
            g.release.send(()).unwrap();
        };
        let (out, ()) = tokio::join!(client, barrier);
        g.task.await.unwrap();
        assert_eq!(out.exit.exit_code, Some(0));
        assert!(out.kept_files.is_empty(), "{:?}", out.kept_files);
        assert_eq!(out.pulled_files.len(), 18);
        for parent in parents {
            assert!(ws.path(parent).is_dir());
            assert_eq!(ws.read(&format!("{parent}/part.txt")), "later child\n");
            assert!(!ws.path(&format!("{parent}/nested/remove.txt")).exists());
            assert_eq!(ws.read(&format!("{parent}/nested/new.txt")), "later new\n");
        }
        println!(
            "transition later poly={poly}: 3 directories, part=\"later child\\n\", new=\"later new\\n\", remove=absent, pulled=18 kept=0"
        );
    }
}

#[tokio::test]
async fn file_to_directory_keeps_conflicting_ancestors_in_both_runners() {
    for poly in [false, true] {
        for mutation in ["edited", "deleted", "directory", "not_returned"] {
            let ws = Workspace::new(&[(".gitignore", "artifacts/\n")]);
            let parent = "artifacts/report.txt";
            let child = "artifacts/report.txt/part.txt";
            ws.write(parent, "old report\n");
            let mut files = vec![];
            if mutation != "not_returned" {
                files.push(delta(parent, None));
            }
            files.extend([
                delta(child, Some("remote child\n")),
                delta("new.txt", Some("new\n")),
            ]);
            let g = gateway(files, other_os()).await;
            let root = ws.root();
            let client = run(g.address, &root, poly, &["fixture"]);
            let edits = async {
                g.arrived.await.unwrap();
                match mutation {
                    "edited" => {
                        let time = std::fs::metadata(ws.path(parent))
                            .unwrap()
                            .modified()
                            .unwrap();
                        ws.write(parent, "local report\n");
                        std::fs::File::options()
                            .write(true)
                            .open(ws.path(parent))
                            .unwrap()
                            .set_modified(time)
                            .unwrap();
                    }
                    "deleted" => std::fs::remove_file(ws.path(parent)).unwrap(),
                    "directory" => {
                        std::fs::remove_file(ws.path(parent)).unwrap();
                        std::fs::create_dir(ws.path(parent)).unwrap();
                    }
                    "not_returned" => {}
                    _ => unreachable!(),
                }
                g.release.send(()).unwrap();
            };
            let (out, ()) = tokio::join!(client, edits);
            g.task.await.unwrap();
            assert_eq!(out.exit.exit_code, Some(0));
            assert_eq!(out.pulled_files, ["new.txt"]);
            assert!(out.kept_files.contains(&child.into()));
            assert!(!ws.path(child).exists());
            match mutation {
                "edited" => assert_eq!(ws.read(parent), "local report\n"),
                "deleted" => assert!(!ws.path(parent).exists()),
                "directory" => assert!(ws.path(parent).is_dir()),
                "not_returned" => assert_eq!(ws.read(parent), "old report\n"),
                _ => unreachable!(),
            }
            println!(
                "transition conflict poly={poly} mutation={mutation}: child=absent kept={:?} pulled={:?}",
                out.kept_files, out.pulled_files
            );
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn file_to_directory_keeps_symlink_ancestors_in_both_runners() {
    for poly in [false, true] {
        for mutation in ["file_link", "directory_link", "unknown_removed"] {
            let ws = Workspace::new(&[(".gitignore", "artifacts/\n")]);
            let parent = "artifacts/report.txt";
            let child = "artifacts/report.txt/part.txt";
            ws.write(parent, "old report\n");
            // Destinations stay outside the fixture root and are never followed by capture.
            let outside = tempfile::tempdir().unwrap();
            let sentinel = outside.path().join("sentinel.txt");
            std::fs::write(&sentinel, "sentinel\n").unwrap();
            if mutation == "unknown_removed" {
                std::fs::remove_file(ws.path(parent)).unwrap();
                std::os::unix::fs::symlink(outside.path(), ws.path(parent)).unwrap();
            }
            let g = gateway(
                vec![delta(parent, None), delta(child, Some("remote child\n"))],
                other_os(),
            )
            .await;
            let root = ws.root();
            let client = run(g.address, &root, poly, &["fixture"]);
            let edits = async {
                g.arrived.await.unwrap();
                std::fs::remove_file(ws.path(parent)).unwrap();
                if mutation != "unknown_removed" {
                    let target = if mutation == "file_link" {
                        sentinel.as_path()
                    } else {
                        outside.path()
                    };
                    std::os::unix::fs::symlink(target, ws.path(parent)).unwrap();
                }
                g.release.send(()).unwrap();
            };
            let (out, ()) = tokio::join!(client, edits);
            g.task.await.unwrap();
            assert_eq!(out.exit.exit_code, Some(0));
            assert!(out.pulled_files.is_empty());
            assert_eq!(out.kept_files, [parent, child]);
            assert!(!ws.path(child).exists());
            assert!(!outside.path().join("part.txt").exists());
            assert_eq!(std::fs::read_to_string(&sentinel).unwrap(), "sentinel\n");
            if mutation == "unknown_removed" {
                assert!(!ws.path(parent).exists());
            } else {
                assert!(
                    std::fs::symlink_metadata(ws.path(parent))
                        .unwrap()
                        .is_symlink()
                );
            }
            println!(
                "transition link poly={poly} mutation={mutation}: child=absent sentinel=\"sentinel\\n\" kept={:?}",
                out.kept_files
            );
        }
    }
}
