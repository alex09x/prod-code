/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::*;
use std::fs::File;
use std::time::{Duration, SystemTime};

#[test]
fn persisted_sessions_reject_invalid_closed_expired_and_mode_changes() {
    let mut file = tempfile::tempfile().unwrap();
    assert_eq!(
        load_cli_stream_session(&mut file, false, false)
            .err()
            .unwrap()
            .to_string(),
        "CLI stream session not found; start it with --reset"
    );
    let initial = load_cli_stream_session(&mut file, true, true).unwrap();
    assert_eq!(
        (
            initial.version,
            initial.borrow_check,
            initial.closed,
            initial.chunks
        ),
        (1, true, false, vec![])
    );
    for (version, closed, updated, mode, expected) in [
        (
            2,
            false,
            0,
            false,
            "persisted CLI stream session version is unsupported; start a new session with --reset",
        ),
        (
            1,
            true,
            0,
            false,
            "CLI stream session is already closed; start a new session with --reset",
        ),
        (
            1,
            false,
            1,
            false,
            "CLI stream session expired; start a new session with --reset",
        ),
        (
            1,
            false,
            0,
            true,
            "borrow-check mode cannot change during a CLI stream session",
        ),
    ] {
        write_cli_stream_session(
            &mut file,
            &CliStreamSessionState {
                version,
                closed,
                updated_at_secs: updated,
                borrow_check: mode,
                chunks: vec!["α\r\n".into()],
            },
        )
        .unwrap();
        assert_eq!(
            load_cli_stream_session(&mut file, false, false)
                .err()
                .unwrap()
                .to_string(),
            expected
        );
    }
    file.set_len(1).unwrap();
    assert_eq!(
        load_cli_stream_session(&mut file, false, false)
            .err()
            .unwrap()
            .to_string(),
        "persisted CLI stream session is invalid; start a new session with --reset"
    );
    write_cli_stream_session(
        &mut file,
        &CliStreamSessionState {
            version: 1,
            chunks: vec!["short".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let loaded = load_cli_stream_session(&mut file, false, false).unwrap();
    assert_eq!(loaded.chunks, vec!["short"]);
    file.set_modified(SystemTime::now() - CLI_STREAM_SESSION_TTL - Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        load_cli_stream_session(&mut file, false, false)
            .err()
            .unwrap()
            .to_string(),
        "CLI stream session expired; start a new session with --reset"
    );
}

#[test]
fn pruning_preserves_active_locked_recent_and_non_session_files() {
    let dir = tempfile::tempdir().unwrap();
    let active = dir.path().join("active.json");
    let old = SystemTime::now() - CLI_STREAM_SESSION_TTL - Duration::from_secs(1);
    let mut locked = None;
    for name in [
        "active.json",
        "locked.json",
        "stale.json",
        "recent.json",
        "keep.txt",
    ] {
        let file = File::create(dir.path().join(name)).unwrap();
        if name != "recent.json" {
            file.set_modified(old).unwrap();
        }
        if name == "locked.json" {
            file.try_lock().unwrap();
            locked = Some(file);
        }
    }
    std::fs::create_dir(dir.path().join("directory.json")).unwrap();
    prune_expired_cli_stream_sessions(dir.path(), &active).unwrap();
    for name in [
        "active.json",
        "locked.json",
        "recent.json",
        "keep.txt",
        "directory.json",
    ] {
        assert!(dir.path().join(name).exists(), "{name}");
    }
    assert!(!dir.path().join("stale.json").exists());
    drop(locked);
    prune_expired_cli_stream_sessions(dir.path(), &active).unwrap();
    assert!(!dir.path().join("locked.json").exists());
}
