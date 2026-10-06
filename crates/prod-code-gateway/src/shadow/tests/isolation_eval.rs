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
use std::time::Duration;

use super::super::overlay::run_overlay;
use super::super::root::overlay_unavailable;
use super::fixtures::{
    delta, in_both_modes, job, output, refused_with, sentinel, snapshot, workspace_with_sibling,
};

/// #440: the overlay run script read its deletions one per line, so `name\n../outside`
/// removed the workspace's sibling although every component of the path is a plain name.
#[tokio::test]
async fn a_newline_in_a_path_never_deletes_outside_the_workspace() {
    let (_root, ws, outside) = workspace_with_sibling();
    std::fs::write(ws.join("name"), "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(&ws);
    for path in [
        "name\n../outside",
        "name\r../outside",
        "tab\there",
        "del\u{7f}",
    ] {
        let files = vec![delta(path, None)];
        in_both_modes(
            || {
                job(
                    &ws,
                    shadow.path(),
                    "newline",
                    files.clone(),
                    &["sh", "-c", "echo ran"],
                )
            },
            |mode, r| {
                assert_eq!(
                    sentinel(&outside).as_deref(),
                    Some("keep\n"),
                    "{mode} {path:?}: the sibling sentinel was deleted ({:?} {})",
                    r.error,
                    output(&r)
                );
                refused_with(mode, &r, "control character");
            },
        )
        .await;
    }
    assert_eq!(snapshot(&ws), before);
}

/// #440: a proposal under a symlinked directory would be written or deleted through it in
/// place and through `rm` in the overlay; both modes refuse it before touching anything.
#[tokio::test]
async fn symlinked_parents_are_refused_before_anything_is_written() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    return;

    let (_root, ws, outside) = workspace_with_sibling();
    symlink(&outside, ws.join("abs")).unwrap();
    symlink("../outside", ws.join("rel")).unwrap();
    std::fs::create_dir_all(ws.join("ci")).unwrap();
    // Inside the workspace, and still refused: in place it writes into `ci`, the overlay
    // hides the link behind a new directory.
    symlink("ci", ws.join("inner")).unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(&ws);
    let out = outside.to_str().unwrap().to_string();
    let argv = ["sh", "-c", "cat \"$1/sentinel\"; ls \"$1\"", "sh", &out];
    for files in [
        vec![delta("abs/sentinel", Some("pwned\n"))],
        vec![delta("abs/sentinel", None)],
        vec![delta("rel/sentinel", None)],
        vec![delta("rel/new/file", Some("pwned\n"))],
        vec![delta("ok.txt", Some("")), delta("inner/x", Some(""))],
    ] {
        in_both_modes(
            || job(&ws, shadow.path(), "symlink-parent", files.clone(), &argv),
            |mode, r| {
                assert_eq!(
                    sentinel(&outside).as_deref(),
                    Some("keep\n"),
                    "{mode} {files:?}: the outside sentinel changed"
                );
                assert!(!outside.join("new").exists(), "{mode} {files:?}");
                refused_with(mode, &r, "symbolic link");
            },
        )
        .await;
        assert_eq!(snapshot(&ws), before, "{files:?}");
    }
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
    assert!(!ws.join("ci/x").exists());
}

/// #440: proposals the two modes applied differently (or only partly) are refused by both.
#[tokio::test]
async fn ambiguous_or_conflicting_proposals_are_refused_in_both_modes() {
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::write(w.join("a.txt"), "base\n").unwrap();
    std::fs::write(w.join("file"), "f\n").unwrap();
    std::fs::create_dir_all(w.join("dir/sub")).unwrap();
    std::fs::write(w.join("dir/sub/x"), "x\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(w);
    let cases: Vec<(Vec<FileDelta>, &str)> = vec![
        (
            vec![delta("a.txt", Some("one\n")), delta("a.txt", None)],
            "more than once",
        ),
        (
            vec![delta("a.txt", None), delta("./a.txt", Some("two\n"))],
            "more than once",
        ),
        (
            vec![delta("a.txt", Some("1\n")), delta("a.txt/", Some("2\n"))],
            "more than once",
        ),
        (
            vec![delta("new", Some("")), delta("new/x", Some(""))],
            "inside",
        ),
        (vec![delta("dir/sub/x", None), delta("dir", None)], "inside"),
        (vec![delta("file/x", Some(""))], "not a directory"),
        (vec![delta("dir", Some(""))], "is a directory"),
        (vec![delta("dir/sub", None)], "is a directory"),
    ];
    let argv = ["sh", "-c", "echo ran > ran.txt; echo ran"];
    for (files, needle) in cases {
        in_both_modes(
            || job(w, shadow.path(), "ambiguous", files.clone(), &argv),
            |mode, r| refused_with(&format!("{mode} {files:?}"), &r, needle),
        )
        .await;
        assert_eq!(snapshot(w), before, "{files:?}");
    }
}

/// A proposed file or deletion replaces a symlink itself in both modes: the target is
/// neither written nor read as the proposed file, and the link is back afterwards.
#[tokio::test]
async fn a_proposed_file_replaces_a_symlink_without_writing_through_it() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    return;

    let (_root, ws, outside) = workspace_with_sibling();
    symlink(outside.join("sentinel"), ws.join("out.txt")).unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(&ws);
    let target = outside.join("sentinel").to_str().unwrap().to_string();
    let argv = [
        "sh",
        "-c",
        "cat out.txt 2>/dev/null || echo missing; cat \"$1\"; test -L out.txt && echo link || echo nolink",
        "sh",
        &target,
    ];
    for (files, expected) in [
        (
            vec![delta("out.txt", Some("proposed\n"))],
            "proposed\nkeep\nnolink\n",
        ),
        (vec![delta("out.txt", None)], "missing\nkeep\nnolink\n"),
    ] {
        in_both_modes(
            || job(&ws, shadow.path(), "symlink", files.clone(), &argv),
            |mode, r| {
                assert_eq!(sentinel(&outside).as_deref(), Some("keep\n"), "{mode}");
                assert_eq!(r.exit_code, Some(0), "{mode}: {:?} {}", r.error, output(&r));
                assert_eq!(output(&r), expected, "{mode} {files:?}");
                assert_eq!(snapshot(&ws), before, "{mode} {files:?}: base not restored");
            },
        )
        .await;
    }
}

/// Names with spaces, backslashes and non-ASCII characters are staged, deleted and
/// restored as they are, and the in-place mode puts the base back exactly: content, mode,
/// modification time, and no directory it created for a proposed file.
#[tokio::test]
async fn unicode_and_spaces_round_trip_and_the_base_is_restored_exactly() {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let ws = tempfile::tempdir().unwrap();
    let w = ws.path();
    std::fs::create_dir_all(w.join("dir with space")).unwrap();
    std::fs::write(w.join("dir with space/ünï cödé ✓.txt"), "base\n").unwrap();
    std::fs::write(w.join(" lead and trail "), "base\n").unwrap();
    std::fs::write(w.join("back\\slash"), "base\n").unwrap();
    let run = w.join("run.sh");
    std::fs::write(&run, "#!/bin/sh\necho base\n").unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&run)
        .unwrap()
        .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_000_000_000))
        .unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let before = snapshot(w);
    let files = vec![
        delta("dir with space/ünï cödé ✓.txt", Some("proposed ✓\n")),
        delta(" lead and trail ", None),
        delta("back\\slash", None),
        FileDelta {
            relative_path: "run.sh".to_string(),
            content: Some(b"#!/bin/sh\necho proposed\n".to_vec()),
            is_executable: true,
        },
        delta("fresh dir/nested/новый файл.txt", Some("new\n")),
    ];
    let script = "cat 'dir with space/ünï cödé ✓.txt'; ./run.sh; \
                  cat 'fresh dir/nested/новый файл.txt'; \
                  test -e ' lead and trail ' && echo lead-kept || echo lead-gone; \
                  test -e 'back\\slash' && echo backslash-kept || echo backslash-gone";
    in_both_modes(
        || {
            job(
                w,
                shadow.path(),
                "unicode",
                files.clone(),
                &["sh", "-c", script],
            )
        },
        |mode, r| {
            assert_eq!(r.exit_code, Some(0), "{mode}: {:?} {}", r.error, output(&r));
            assert_eq!(
                output(&r),
                "proposed ✓\nproposed\nnew\nlead-gone\nbackslash-gone\n",
                "{mode}"
            );
            assert_eq!(snapshot(w), before, "{mode}: base not restored exactly");
        },
    )
    .await;
}

/// #440: the request's env cannot point the run script at another deletion list or
/// directory; its own variables are set last.
#[tokio::test]
async fn request_env_cannot_replace_the_run_script_variables() {
    if let Some(reason) = overlay_unavailable() {
        eprintln!("skipped: {reason}");
        return;
    }
    let (root, ws, outside) = workspace_with_sibling();
    std::fs::write(ws.join("a.txt"), "base\n").unwrap();
    let list = root.path().join("crafted-delete.txt");
    std::fs::write(&list, "../outside/sentinel\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let mut j = job(
        &ws,
        shadow.path(),
        "env",
        vec![delta("a.txt", Some("proposed\n"))],
        &["sh", "-c", "pwd; cat a.txt"],
    );
    j.env = vec![
        (
            "SHADOW_DELETE".to_string(),
            list.to_str().unwrap().to_string(),
        ),
        ("SHADOW_SUBDIR".to_string(), "../outside".to_string()),
    ];
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let r = run_overlay(j, rx).await;
    assert_eq!(
        sentinel(&outside).as_deref(),
        Some("keep\n"),
        "{}",
        output(&r)
    );
    assert_eq!(r.exit_code, Some(0), "{:?} {}", r.error, output(&r));
    assert_eq!(output(&r), format!("{}\nproposed\n", ws.display()));
    assert_eq!(std::fs::read_to_string(ws.join("a.txt")).unwrap(), "base\n");
}
