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
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::super::overlay::run_overlay;
use super::super::root::overlay_unavailable;
use super::super::sccache::{effective_var, sccache_config_content};
use super::super::staging::check_files;
use super::fixtures::{SCHEDULER_TOML, delta, job, output, refused, refused_with, verdict};

#[test]
fn a_proposed_sccache_config_inside_the_workspace_is_what_counts() {
    let ws = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(ws.path().join("ci")).unwrap();
    let path = ws.path().join("ci/sccache.toml");
    let p = path.to_str().unwrap();
    let vars = [("SCCACHE_CONF", p), ("SCCACHE_CLIENT_SIDE", "1")];
    std::fs::write(&path, "[cache.disk]\ndir = \"/tmp/c\"\n").unwrap();
    verdict(&vars, ws.path(), &[]).unwrap();
    refused(
        verdict(
            &vars,
            ws.path(),
            &[delta("ci/sccache.toml", Some(SCHEDULER_TOML))],
        ),
        "dist.scheduler_url",
    );
    refused(
        verdict(
            &vars,
            ws.path(),
            &[delta("./ci/sccache.toml", Some("[dist"))],
        ),
        "is not valid TOML",
    );
    std::fs::write(&path, SCHEDULER_TOML).unwrap();
    refused(verdict(&vars, ws.path(), &[]), "dist.scheduler_url");
    verdict(
        &vars,
        ws.path(),
        &[delta("ci/sccache.toml", Some("client_side_mode = true\n"))],
    )
    .unwrap();
    verdict(&vars, ws.path(), &[delta("ci/sccache.toml", None)]).unwrap();
    // Content under a deleted directory is ambiguous and never reaches the view (#440).
    let nested = [
        delta("ci/sccache.toml", Some(SCHEDULER_TOML)),
        delta("ci", None),
    ];
    let why = check_files(ws.path(), &nested).unwrap_err();
    assert!(why.contains("one lies inside the other"), "{why}");
    // Other proposed files leave the one on disk in force.
    refused(
        verdict(&vars, ws.path(), &[delta("ci/other.toml", Some(""))]),
        "dist.scheduler_url",
    );
}

#[test]
fn aliases_of_a_proposed_sccache_config_resolve_as_in_the_hypothesis() {
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    return;

    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let (w, o) = (ws.path(), outside.path());
    let safe = "[cache.disk]\ndir = \"/tmp/c\"\n";
    std::fs::create_dir_all(w.join("ci")).unwrap();
    std::fs::write(w.join("sccache.toml"), safe).unwrap();
    std::fs::write(w.join("ci/sccache.toml"), safe).unwrap();
    let check = |conf: &Path, files: &[FileDelta]| {
        verdict(
            &[
                ("SCCACHE_CONF", conf.to_str().unwrap()),
                ("SCCACHE_CLIENT_SIDE", "1"),
            ],
            w,
            files,
        )
    };
    let scheduler = |rel: &str| vec![delta(rel, Some(SCHEDULER_TOML))];

    // A parent component: the proposed file is the one the path resolves to.
    let dotdot = w.join("ci/../sccache.toml");
    check(&dotdot, &[]).unwrap();
    refused(
        check(&dotdot, &scheduler("sccache.toml")),
        "dist.scheduler_url",
    );
    // Through a directory only the hypothesis creates; without it the path does not exist.
    let staged = w.join("new/../sccache.toml");
    check(&staged, &scheduler("sccache.toml")).unwrap();
    refused(
        check(
            &staged,
            &[
                delta("new/keep.txt", Some("")),
                delta("sccache.toml", Some(SCHEDULER_TOML)),
            ],
        ),
        "dist.scheduler_url",
    );

    // Symlinks outside the workspace into it, to the file and to its directory.
    symlink(w.join("ci/sccache.toml"), o.join("into.toml")).unwrap();
    symlink(w.join("ci"), o.join("intodir")).unwrap();
    for conf in [o.join("into.toml"), o.join("intodir/sccache.toml")] {
        check(&conf, &[]).unwrap();
        refused(
            check(&conf, &scheduler("ci/sccache.toml")),
            "dist.scheduler_url",
        );
    }
    // Content under a deleted directory is refused before any view is built (#440).
    let nested = [
        delta("ci/sccache.toml", Some(SCHEDULER_TOML)),
        delta("ci", None),
    ];
    assert!(check_files(w, &nested).is_err());
    // A symlink inside the workspace to another file in it.
    symlink("ci/sccache.toml", w.join("link.toml")).unwrap();
    refused(
        check(&w.join("link.toml"), &scheduler("ci/sccache.toml")),
        "dist.scheduler_url",
    );
    // The workspace named through a symlink is mounted where it resolves to.
    symlink(w, o.join("ws")).unwrap();
    refused(
        verdict(
            &[
                ("SCCACHE_CONF", w.join("sccache.toml").to_str().unwrap()),
                ("SCCACHE_CLIENT_SIDE", "1"),
            ],
            &o.join("ws"),
            &scheduler("sccache.toml"),
        ),
        "dist.scheduler_url",
    );

    // A config read through a symlinked directory is the file it points into; proposals
    // under that directory are refused before any view is built (#440).
    symlink("ci", w.join("lnk")).unwrap();
    std::fs::write(w.join("ci/sccache.toml"), SCHEDULER_TOML).unwrap();
    refused(check(&w.join("ci/sccache.toml"), &[]), "dist.scheduler_url");
    refused(
        check(&w.join("lnk/sccache.toml"), &[]),
        "dist.scheduler_url",
    );
    for files in [
        [delta("lnk/sccache.toml", None)],
        [delta("lnk/other.toml", Some(""))],
    ] {
        let why = check_files(w, &files).unwrap_err();
        assert!(why.contains("symbolic link"), "{why}");
    }

    // A proposed file replaces a symlink as a regular file; its target no longer counts.
    std::fs::write(o.join("out.toml"), SCHEDULER_TOML).unwrap();
    symlink(o.join("out.toml"), w.join("out.toml")).unwrap();
    let out = w.join("out.toml");
    refused(check(&out, &[]), "dist.scheduler_url");
    check(&out, &[delta("out.toml", Some(safe))]).unwrap();
    refused(
        check(&o.join("out.toml"), &[delta("out.toml", Some(safe))]),
        "dist.scheduler_url",
    );
    std::fs::write(o.join("out.toml"), safe).unwrap();
    check(&out, &[]).unwrap();
    refused(check(&out, &scheduler("out.toml")), "dist.scheduler_url");
    // Deleting the symlink removes the link, not its target.
    std::fs::write(o.join("out.toml"), SCHEDULER_TOML).unwrap();
    check(&out, &[delta("out.toml", None)]).unwrap();
    refused(
        check(&o.join("out.toml"), &[delta("out.toml", None)]),
        "dist.scheduler_url",
    );

    // A symlink loop is refused rather than guessed.
    symlink("loop", w.join("loop")).unwrap();
    refused(
        check(&w.join("loop"), &[]),
        "too many levels of symbolic links",
    );
}

#[tokio::test]
async fn aliased_sccache_configs_read_what_the_overlay_command_reads() {
    if let Some(reason) = overlay_unavailable() {
        eprintln!("skipped: {reason}");
        return;
    }
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    #[cfg(not(unix))]
    return;

    let ws = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let (w, o) = (ws.path(), outside.path());
    std::fs::create_dir_all(w.join("ci")).unwrap();
    std::fs::write(w.join("sccache.toml"), "# disk root\n").unwrap();
    std::fs::write(w.join("ci/sccache.toml"), "# disk ci\n").unwrap();
    std::fs::write(o.join("outside.toml"), "# outside\n").unwrap();
    symlink("ci", w.join("lnk")).unwrap();
    symlink("ci/sccache.toml", w.join("link.toml")).unwrap();
    symlink(o.join("outside.toml"), w.join("out.toml")).unwrap();
    symlink(w.join("ci/sccache.toml"), o.join("into.toml")).unwrap();
    symlink(w.join("ci"), o.join("intodir")).unwrap();
    let root = || delta("sccache.toml", Some("# proposed root\n"));
    let ci = || delta("ci/sccache.toml", Some("# proposed ci\n"));
    let cases: Vec<(PathBuf, Vec<FileDelta>)> = vec![
        (w.join("ci/../sccache.toml"), vec![root()]),
        (w.join("new/../sccache.toml"), vec![root()]),
        (
            w.join("new/../sccache.toml"),
            vec![delta("new/keep.txt", Some("")), root()],
        ),
        (o.join("into.toml"), vec![ci()]),
        (o.join("intodir/sccache.toml"), vec![ci()]),
        (w.join("link.toml"), vec![ci()]),
        (w.join("out.toml"), vec![]),
        (
            w.join("out.toml"),
            vec![delta("out.toml", Some("# proposed out\n"))],
        ),
        (w.join("out.toml"), vec![delta("out.toml", None)]),
        (o.join("outside.toml"), vec![delta("out.toml", None)]),
        (w.join("lnk/sccache.toml"), vec![]),
    ];
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let argv = [
        "sh",
        "-c",
        "cat \"$SCCACHE_CONF\" 2>/dev/null || echo missing",
    ];
    // Ambiguous or symlink-parent proposals never run, whatever the config path (#440).
    let refused_cases: Vec<(PathBuf, Vec<FileDelta>)> = vec![
        (
            o.join("intodir/sccache.toml"),
            vec![ci(), delta("ci", None)],
        ),
        (
            w.join("lnk/sccache.toml"),
            vec![delta("lnk/other.toml", Some(""))],
        ),
        (
            w.join("ci/sccache.toml"),
            vec![delta("lnk/sccache.toml", None)],
        ),
    ];
    for (i, (conf, files)) in refused_cases.into_iter().enumerate() {
        let mut j = job(w, shadow.path(), &format!("refused-{i}"), files, &argv);
        j.env = vec![(
            "SCCACHE_CONF".to_string(),
            conf.to_str().unwrap().to_string(),
        )];
        let r = run_overlay(j, rx.clone()).await;
        refused_with(&format!("refused case {i}"), &r, "invalid hypothesis: ");
    }
    for (i, (conf, files)) in cases.into_iter().enumerate() {
        let expected = match sccache_config_content(&conf, w, &files).unwrap() {
            Some(bytes) => String::from_utf8(bytes).unwrap(),
            None => "missing\n".to_string(),
        };
        let what = format!("case {i}: {}", conf.display());
        let mut j = job(w, shadow.path(), &format!("alias-{i}"), files, &argv);
        j.env = vec![(
            "SCCACHE_CONF".to_string(),
            conf.to_str().unwrap().to_string(),
        )];
        let r = run_overlay(j, rx.clone()).await;
        assert_eq!(r.exit_code, Some(0), "{what}: {:?}", r.error);
        assert_eq!(output(&r), expected, "{what}");
    }
    assert_eq!(
        std::fs::read_to_string(o.join("outside.toml")).unwrap(),
        "# outside\n"
    );
    assert_eq!(
        std::fs::read_to_string(w.join("ci/sccache.toml")).unwrap(),
        "# disk ci\n"
    );
}

#[test]
fn effective_env_is_the_request_over_the_gateway() {
    let env = vec![
        ("PATH".to_string(), "/first".to_string()),
        ("PATH".to_string(), "/last".to_string()),
    ];
    assert_eq!(effective_var(&env, "PATH"), Some(OsString::from("/last")));
    assert_eq!(effective_var(&[], "PATH"), std::env::var_os("PATH"));
    // run_overlay sets SCCACHE_CLIENT_SIDE=1 unless the request overrides it.
    assert_eq!(
        effective_var(&[], "SCCACHE_CLIENT_SIDE"),
        Some(OsString::from("1"))
    );
    let off = vec![("SCCACHE_CLIENT_SIDE".to_string(), String::new())];
    assert_eq!(
        effective_var(&off, "SCCACHE_CLIENT_SIDE"),
        Some(OsString::new())
    );
}
