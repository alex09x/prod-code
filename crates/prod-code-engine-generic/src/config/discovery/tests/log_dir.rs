/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The probe points npm's log directory at a private, probe-only target (#1794).
//!
//! npm 11 runs its log cleanup even with `logs-max=0`: it lists `logs-dir` (default
//! `<cache>/_logs`) and deletes every `*-debug-*.log` there. The fake npm below mimics
//! that against fixture logs, so the user's real npm and logs are never touched.

use super::*;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Writes a fake npm that records where it would clean logs and then cleans there the
/// way npm does with `logs-max=0`.
fn log_cleaning_npm(dir: &Path, record: &Path, root: &Path) -> PathBuf {
    let npm = dir.join("cleaning_npm");
    make_executable_script(
        &npm,
        &format!(
            "logs_dir=\"\"\n\
             for a in \"$@\"; do case \"$a\" in --logs-dir=*) logs_dir=\"${{a#--logs-dir=}}\";; esac; done\n\
             [ -n \"$logs_dir\" ] || logs_dir=\"$npm_config_logs_dir\"\n\
             [ -n \"$logs_dir\" ] || logs_dir=\"$HOME/.npm/_logs\"\n\
             rec=\"{record}\"\n\
             echo \"dir=$logs_dir\" >> \"$rec\"\n\
             echo \"env=$npm_config_logs_dir\" >> \"$rec\"\n\
             echo \"parent=$(ls -ld \"$(dirname \"$logs_dir\")\" 2>&1)\" >> \"$rec\"\n\
             if [ -e \"$logs_dir\" ]; then echo child=exists >> \"$rec\"; else echo child=absent >> \"$rec\"; fi\n\
             for f in \"$logs_dir\"/*-debug-*.log; do [ -f \"$f\" ] && rm -f \"$f\"; done\n\
             echo \"{root}\"",
            record = record.to_string_lossy(),
            root = root.to_string_lossy(),
        ),
    );
    npm
}

fn fixture_log(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let log = dir.join("2026-01-01T00_00_00_000Z-debug-0.log");
    std::fs::write(&log, "user log").unwrap();
    log
}

fn field<'a>(record: &'a str, key: &str) -> Vec<&'a str> {
    record
        .lines()
        .filter_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .collect()
}

#[cfg(unix)]
#[test]
fn probe_never_cleans_the_user_log_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("global_modules");
    std::fs::create_dir_all(&root).unwrap();
    let home = temp.path().join("home");
    let default_log = fixture_log(&home.join(".npm").join("_logs"));
    let configured_logs = temp.path().join("configured_logs");
    let configured_log = fixture_log(&configured_logs);
    let record = temp.path().join("record.txt");
    let npm = log_cleaning_npm(temp.path(), &record, &root);

    // The default log directory under the cache, and one the user configured through
    // the environment: neither may be listed or cleaned.
    let first = probe_npm_root(
        &npm,
        Duration::from_secs(5),
        [(OsString::from("HOME"), home.clone().into_os_string())],
    );
    let second = probe_npm_root(
        &npm,
        Duration::from_secs(5),
        [
            (OsString::from("HOME"), home.clone().into_os_string()),
            (
                OsString::from("npm_config_logs_dir"),
                configured_logs.clone().into_os_string(),
            ),
        ],
    );
    assert_eq!(first, Some(root.clone()));
    assert_eq!(second, Some(root));
    assert!(
        default_log.exists(),
        "default user log must survive the probe"
    );
    assert!(
        configured_log.exists(),
        "configured user log must survive the probe"
    );

    let record = std::fs::read_to_string(&record).unwrap();
    let dirs = field(&record, "dir");
    let envs = field(&record, "env");
    assert_eq!(dirs.len(), 2, "{record}");
    assert_eq!(dirs, envs, "CLI and environment must name the same target");
    assert_ne!(dirs[0], dirs[1], "each probe gets its own private target");
    for dir in &dirs {
        let dir = Path::new(dir);
        assert!(!dir.starts_with(&home) && !dir.starts_with(&configured_logs));
        let parent = dir.parent().unwrap();
        assert!(
            !parent.exists(),
            "private probe directory {parent:?} must be removed after the probe"
        );
    }
    for listing in field(&record, "parent") {
        assert!(
            listing.starts_with("drwx------"),
            "probe log parent must be private to the owner: {listing}"
        );
    }
    assert_eq!(field(&record, "child"), ["absent", "absent"]);
}
