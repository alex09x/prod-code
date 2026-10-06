/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::FileDelta;

use super::super::in_place::{FillFault, inject_fill_fault, run_in_place};
use super::super::overlay::clean;
use super::fixtures::{
    delta, injected, job, output, refused_with, sentinel, snapshot, workspace_with_sibling,
};

#[tokio::test]
async fn in_place_hypotheses_restore_the_workspace() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let shadow = tempfile::tempdir().unwrap();
    let argv = ["sh", "-c", "cat a.txt 2>&1; ls"];
    let one = run_in_place(
        job(
            ws.path(),
            shadow.path(),
            "one",
            vec![delta("a.txt", Some("one\n")), delta("b.txt", Some("new\n"))],
            &argv,
        ),
        rx.clone(),
    )
    .await;
    assert_eq!(one.exit_code, Some(0), "{:?}", one.error);
    assert!(output(&one).contains("one") && output(&one).contains("b.txt"));
    let two = run_in_place(
        job(
            ws.path(),
            shadow.path(),
            "two",
            vec![delta("a.txt", None)],
            &argv,
        ),
        rx,
    )
    .await;
    assert_eq!(two.exit_code, Some(0));
    assert!(!output(&two).contains("base"));
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "base\n"
    );
    assert!(!ws.path().join("b.txt").exists());
}

#[tokio::test]
async fn in_place_restore_never_writes_through_a_replaced_parent() {
    let (_root, ws, outside) = workspace_with_sibling();
    std::fs::create_dir_all(ws.join("sub")).unwrap();
    std::fs::write(ws.join("sub/a.txt"), "base\n").unwrap();
    let out = outside.to_str().unwrap().to_string();
    let script = "mv sub sub.moved && ln -s \"$1\" sub && cp sub.moved/a.txt \"$1/a.txt\"";
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let r = run_in_place(
        job(
            &ws,
            shadow.path(),
            "swap",
            vec![delta("sub/a.txt", Some("proposed\n"))],
            &["sh", "-c", script, "sh", &out],
        ),
        rx,
    )
    .await;
    assert_eq!(r.exit_code, Some(0), "{:?} {}", r.error, output(&r));
    assert_eq!(
        std::fs::read_to_string(outside.join("a.txt")).unwrap(),
        "proposed\n",
        "the restore wrote through the symlink"
    );
    assert_eq!(sentinel(&outside).as_deref(), Some("keep\n"));
    // Reported rather than logged only: the command's exit 0 is not a clean pass.
    let why = r.error.as_deref().unwrap_or_default();
    assert!(
        why.contains("the workspace could not be restored")
            && why.contains("a parent is no longer a real directory"),
        "{why}"
    );
    assert!(!clean(&r));
}

#[tokio::test]
async fn a_failed_write_rolls_back_to_the_original() {
    use std::io::Write;
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::write(w.join("a.txt"), "base\n").unwrap();
    std::fs::write(w.join("b.txt"), "base b\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(w);
    let (_tx, rx) = tokio::sync::watch::channel(false);
    type File = std::fs::File;
    let partial: fn() -> FillFault = || {
        Box::new(|file: &mut File, bytes: &[u8]| {
            file.write_all(&bytes[..3])?;
            Err(injected("no space left on device"))
        })
    };
    let executable = FileDelta {
        relative_path: "a.txt".to_string(),
        content: Some(b"proposed\n".to_vec()),
        is_executable: true,
    };
    let cases: Vec<(&str, Vec<FileDelta>, FillFault)> = vec![
        (
            "partial",
            vec![delta("a.txt", Some("proposed content\n"))],
            partial(),
        ),
        (
            "nothing written",
            vec![delta("a.txt", Some("proposed\n"))],
            Box::new(|_: &mut File, _: &[u8]| Err(injected("I/O error"))),
        ),
        (
            "chmod",
            vec![executable],
            Box::new(|file: &mut File, bytes: &[u8]| {
                file.write_all(bytes)?;
                Err(injected("chmod refused"))
            }),
        ),
        (
            "second stage",
            vec![
                delta("b.txt", Some("proposed b\n")),
                delta("new dir/sub/a.txt", Some("proposed\n")),
            ],
            // The first file is written in full, the second fails part-way.
            Box::new(move |file: &mut File, bytes: &[u8]| {
                inject_fill_fault(partial());
                file.write_all(bytes)
            }),
        ),
        (
            "after a deletion",
            vec![delta("b.txt", None), delta("a.txt", Some("proposed\n"))],
            partial(),
        ),
    ];
    for (name, files, fault) in cases {
        inject_fill_fault(fault);
        let r = run_in_place(
            job(w, shadow.path(), name, files, &["sh", "-c", "echo ran"]),
            rx.clone(),
        )
        .await;
        refused_with(name, &r, "cannot write ");
        let why = r.error.as_deref().unwrap_or_default();
        assert!(why.contains("injected: "), "{name}: {why}");
        assert!(!why.contains("could not be restored"), "{name}: {why}");
        assert_eq!(snapshot(w), before, "{name}: the base was not restored");
    }
}

#[tokio::test]
async fn a_failed_write_never_removes_a_file_someone_else_put_there() {
    use std::io::Write;
    let ws = tempfile::tempdir().unwrap();
    let path = ws.path().join("a.txt");
    std::fs::write(&path, "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let replaced = path.clone();
    inject_fill_fault(Box::new(move |file, bytes| {
        file.write_all(&bytes[..2])?;
        std::fs::remove_file(&replaced)?;
        std::fs::write(&replaced, "concurrent\n")?;
        Err(injected("interrupted"))
    }));
    let r = run_in_place(
        job(
            ws.path(),
            shadow.path(),
            "replaced",
            vec![delta("a.txt", Some("proposed\n"))],
            &["sh", "-c", "echo ran"],
        ),
        rx,
    )
    .await;
    refused_with("replaced", &r, "cannot write ");
    let why = r.error.as_deref().unwrap_or_default();
    assert!(
        why.contains("the workspace could not be restored")
            && why.contains(&format!("{}: file changed", path.display())),
        "{why}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "concurrent\n");
}

#[tokio::test]
async fn restore_problems_are_returned_and_never_count_as_clean() {
    let ws = tempfile::tempdir().unwrap();
    let path = ws.path().join("a.txt");
    std::fs::write(&path, "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let files = vec![delta("a.txt", Some("proposed\n"))];
    let edits = job(
        ws.path(),
        shadow.path(),
        "edits",
        files.clone(),
        &["sh", "-c", "echo edited > a.txt"],
    );
    let r = run_in_place(edits, rx.clone()).await;
    assert_eq!(r.exit_code, Some(0), "{:?} {}", r.error, output(&r));
    let why = r.error.as_deref().unwrap_or_default();
    assert_eq!(
        why,
        format!(
            "the workspace could not be restored: {}: file changed while the hypothesis \
             ran; not restored",
            path.display()
        )
    );
    assert!(!clean(&r));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "edited\n");
    let reads = job(ws.path(), shadow.path(), "reads", files, &["cat", "a.txt"]);
    let r = run_in_place(reads, rx).await;
    assert!(clean(&r), "{:?}", r.error);
    assert_eq!(output(&r), "proposed\n");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "edited\n");
}

#[tokio::test]
async fn created_directory_cleanup_never_follows_a_replaced_parent() {
    let (_root, ws, outside) = workspace_with_sibling();
    std::fs::create_dir(outside.join("sub")).unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let out = outside.to_str().unwrap().to_string();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let r = run_in_place(
        job(
            &ws,
            shadow.path(),
            "directory-swap",
            vec![delta("new/sub/a.txt", Some("proposed\n"))],
            &["sh", "-c", "mv new saved && ln -s \"$1\" new", "sh", &out],
        ),
        rx,
    )
    .await;
    assert_eq!(r.exit_code, Some(0), "{:?}", r.error);
    assert!(r.error.is_some(), "restore failure must be reported");
    assert!(
        outside.join("sub").is_dir(),
        "cleanup deleted an outside directory through the replaced parent"
    );
    assert_eq!(sentinel(&outside).as_deref(), Some("keep\n"));
}
