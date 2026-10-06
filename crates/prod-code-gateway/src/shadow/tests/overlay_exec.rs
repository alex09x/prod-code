/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::super::overlay::run_overlay;
use super::super::root::overlay_unavailable;
use super::fixtures::{delta, job, output};

#[tokio::test]
async fn run_overlay_falls_back_to_disk_when_ram_staging_fails() {
    if overlay_unavailable().is_some() {
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let ram_shadow_file = tempfile::NamedTempFile::new().unwrap();
    let bad_ram_shadow_root = ram_shadow_file.path().to_path_buf();
    let disk_shadow = tempfile::tempdir().unwrap();
    let ran_in_ram = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let mut test_job = job(
        ws.path(),
        &bad_ram_shadow_root,
        "fallback_hyp",
        vec![delta("a.txt", Some("fallback content\n"))],
        &["sh", "-c", "cat a.txt"],
    );
    test_job.fallback_shadow_root = Some(disk_shadow.path().to_path_buf());
    test_job.ran_in_ram = Some(ran_in_ram.clone());

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let res = run_overlay(test_job, rx).await;
    assert_eq!(res.exit_code, Some(0), "{:?} {}", res.error, output(&res));
    assert!(output(&res).contains("fallback content"));
    assert!(
        !ran_in_ram.load(std::sync::atomic::Ordering::Relaxed),
        "ran_in_ram must be false after falling back to disk overlay"
    );
}

#[tokio::test]
async fn run_overlay_preserves_cancellation_during_fallback() {
    if overlay_unavailable().is_some() {
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let ram_shadow_file = tempfile::NamedTempFile::new().unwrap();
    let bad_ram_shadow_root = ram_shadow_file.path().to_path_buf();
    let disk_shadow = tempfile::tempdir().unwrap();
    let ran_in_ram = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let mut test_job = job(
        ws.path(),
        &bad_ram_shadow_root,
        "cancelled_hyp",
        vec![delta("a.txt", Some("fallback content\n"))],
        &["sh", "-c", "cat a.txt"],
    );
    test_job.fallback_shadow_root = Some(disk_shadow.path().to_path_buf());
    test_job.ran_in_ram = Some(ran_in_ram.clone());

    let (tx, rx) = tokio::sync::watch::channel(false);
    let _ = tx.send(true);
    let res = run_overlay(test_job, rx).await;
    assert_eq!(res.exit_code, None);
    assert_eq!(res.error.as_deref(), Some("cancelled: the client left"));
}

#[tokio::test]
async fn run_overlay_does_not_retry_when_command_exits_253() {
    if overlay_unavailable().is_some() {
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let ram_shadow = tempfile::tempdir().unwrap();
    let disk_shadow = tempfile::tempdir().unwrap();
    let ran_in_ram = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let mut test_job = job(
        ws.path(),
        ram_shadow.path(),
        "exit_253_hyp",
        vec![delta("a.txt", Some("overlay content\n"))],
        &["sh", "-c", "cat a.txt > /dev/null; exit 253"],
    );
    test_job.fallback_shadow_root = Some(disk_shadow.path().to_path_buf());
    test_job.ran_in_ram = Some(ran_in_ram.clone());

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let res = run_overlay(test_job, rx).await;
    assert_eq!(res.exit_code, Some(253));
    assert!(
        ran_in_ram.load(std::sync::atomic::Ordering::Relaxed),
        "command exiting 253 after successful mount must not trigger disk fallback"
    );
}

#[tokio::test]
async fn run_overlay_isolates_mount_status_from_user_command() {
    if overlay_unavailable().is_some() {
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let ram_shadow = tempfile::tempdir().unwrap();
    let disk_shadow = tempfile::tempdir().unwrap();
    let ran_in_ram = Arc::new(std::sync::atomic::AtomicBool::new(true));

    // The user command tries to inspect SHADOW_MOUNT_STATUS and SHADOW_CONTROL_DIR
    let mut test_job = job(
        ws.path(),
        ram_shadow.path(),
        "unshare_isolation_hyp",
        vec![delta("a.txt", Some("overlay content\n"))],
        &[
            "sh",
            "-c",
            "test -z \"$SHADOW_MOUNT_STATUS\" && test -z \"$SHADOW_CONTROL_DIR\"",
        ],
    );
    test_job.fallback_shadow_root = Some(disk_shadow.path().to_path_buf());
    test_job.ran_in_ram = Some(ran_in_ram.clone());

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let res = run_overlay(test_job, rx).await;
    assert_eq!(res.exit_code, Some(0), "{:?} {}", res.error, output(&res));
    assert!(
        ran_in_ram.load(std::sync::atomic::Ordering::Relaxed),
        "command must run in RAM and not trigger fallback"
    );
}

#[tokio::test]
async fn overlay_hypotheses_run_in_parallel_and_leave_the_workspace_untouched() {
    if let Some(reason) = overlay_unavailable() {
        eprintln!("skipped: {reason}");
        return;
    }
    let ws = tempfile::tempdir().unwrap();
    std::fs::write(ws.path().join("a.txt"), "base\n").unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let argv = ["sh", "-c", "cat a.txt 2>&1; ls; echo made > made.txt"];
    let one = run_overlay(
        job(
            ws.path(),
            shadow.path(),
            "one",
            vec![delta("a.txt", Some("one\n")), delta("b.txt", Some("new\n"))],
            &argv,
        ),
        rx.clone(),
    );
    let two = run_overlay(
        job(
            ws.path(),
            shadow.path(),
            "two",
            vec![delta("a.txt", None)],
            &argv,
        ),
        rx,
    );
    let (one, two) = tokio::join!(one, two);
    assert_eq!(one.exit_code, Some(0), "{:?} {}", one.error, output(&one));
    assert!(output(&one).contains("one") && output(&one).contains("b.txt"));
    assert_eq!(two.exit_code, Some(0), "{:?} {}", two.error, output(&two));
    assert!(!output(&two).contains("base") && !output(&two).contains("a.txt\n"));
    assert_eq!(
        std::fs::read_to_string(ws.path().join("a.txt")).unwrap(),
        "base\n"
    );
    assert!(!ws.path().join("b.txt").exists());
    assert!(!ws.path().join("made.txt").exists());
    // The upper directories are removed after each hypothesis.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(std::fs::read_dir(shadow.path()).unwrap().count(), 0);
}

/// Namespace root is only for mounting: it must have the same refusal of a 0444 file as an
/// ordinary command, while a writable file remains writable inside the isolated overlay.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn overlay_commands_obey_read_only_permissions() {
    use std::os::unix::fs::PermissionsExt;

    if let Some(reason) = overlay_unavailable() {
        panic!("this test needs overlay shadows: {reason}");
    }
    let ws = tempfile::tempdir().unwrap();
    let shadow = tempfile::tempdir().unwrap();
    let locked = ws.path().join("locked.txt");
    let writable = ws.path().join("writable.txt");
    std::fs::write(&locked, "locked base\n").unwrap();
    std::fs::write(&writable, "writable base\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o444)).unwrap();

    let ordinary = std::process::Command::new("sh")
        .args(["-c", "printf changed > locked.txt"])
        .current_dir(ws.path())
        .output()
        .unwrap();
    assert!(
        !ordinary.status.success(),
        "ordinary execution unexpectedly wrote a 0444 file"
    );
    assert_eq!(std::fs::read_to_string(&locked).unwrap(), "locked base\n");

    let (_tx, rx) = tokio::sync::watch::channel(false);
    let denied = run_overlay(
        job(
            ws.path(),
            shadow.path(),
            "locked",
            vec![],
            &["sh", "-c", "printf changed > locked.txt"],
        ),
        rx.clone(),
    )
    .await;
    assert_ne!(
        denied.exit_code,
        Some(0),
        "overlay bypassed the ordinary 0444 refusal: {}",
        output(&denied)
    );
    assert_eq!(std::fs::read_to_string(&locked).unwrap(), "locked base\n");

    let writable = run_overlay(
        job(
            ws.path(),
            shadow.path(),
            "writable",
            vec![],
            &[
                "sh",
                "-c",
                "printf overlay > writable.txt; cat writable.txt",
            ],
        ),
        rx,
    )
    .await;
    assert_eq!(
        writable.exit_code,
        Some(0),
        "{:?} {}",
        writable.error,
        output(&writable)
    );
    assert_eq!(output(&writable), "overlay");
    assert_eq!(
        std::fs::read_to_string(ws.path().join("writable.txt")).unwrap(),
        "writable base\n"
    );
}

/// Runs the existing multi-repository rollback test in the source-built gateway's real
/// overlay. Its 0444 fixture must refuse the write just as it does outside the namespace.
#[cfg(target_os = "linux")]
#[tokio::test]
#[ignore = "runs the multi-repository atomic rename scenario through a real overlay"]
async fn overlay_runs_the_atomic_rename_rollback_scenario() {
    if let Some(reason) = overlay_unavailable() {
        panic!("this test needs overlay shadows: {reason}");
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the gateway crate is below the workspace root");
    let shadow = tempfile::tempdir().unwrap();
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let mut hypothesis = job(
        workspace,
        shadow.path(),
        "atomic-rename",
        vec![],
        &[
            "cargo",
            "test",
            "-p",
            "prod-code-mcp",
            "a_rename_across_repositories_is_written_in_all_or_none",
            "--",
            "--nocapture",
        ],
    );
    hypothesis.timeout = Duration::from_secs(600);
    let result = run_overlay(hypothesis, rx).await;
    assert_eq!(
        result.exit_code,
        Some(0),
        "{:?} {}",
        result.error,
        output(&result)
    );
    assert!(
        output(&result)
            .contains("test a_rename_across_repositories_is_written_in_all_or_none ... ok"),
        "{}",
        output(&result)
    );
}
