/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use prod_code_protocol::{FileDelta, ShadowHypothesisResult};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Weak;
use std::time::{Duration, Instant};

use super::super::in_place::lock::{in_place_key, in_place_locks};
use super::super::in_place::run_in_place;
use super::super::overlay::run_overlay;
use super::super::root::overlay_unavailable;
use super::super::sccache::sccache_client_side;
use super::super::types::Job;

pub(crate) const SCHEDULER_TOML: &str = "[dist]\nscheduler_url = \"https://scheduler.invalid\"\n[dist.auth]\ntype = \"token\"\ntoken = \"hunter2\"\n";

pub(crate) fn job(
    workspace: &Path,
    shadow_root: &Path,
    name: &str,
    files: Vec<FileDelta>,
    argv: &[&str],
) -> Job {
    Job {
        name: name.to_string(),
        files,
        argv: argv.iter().map(|s| s.to_string()).collect(),
        env: Vec::new(),
        timeout: Duration::from_secs(30),
        tail_limit: 4096,
        workspace: workspace.to_path_buf(),
        subdir: String::new(),
        shadow_root: shadow_root.to_path_buf(),
        fallback_shadow_root: None,
        ran_in_ram: None,
        nonce: 7,
    }
}

pub(crate) fn delta(path: &str, content: Option<&str>) -> FileDelta {
    FileDelta {
        relative_path: path.to_string(),
        content: content.map(|c| c.as_bytes().to_vec()),
        is_executable: false,
    }
}

pub(crate) fn output(result: &ShadowHypothesisResult) -> String {
    String::from_utf8_lossy(result.output_tail.as_deref().unwrap_or_default()).into_owned()
}

pub(crate) fn verdict(
    vars: &[(&str, &str)],
    workspace: &Path,
    files: &[FileDelta],
) -> std::result::Result<(), String> {
    let var = |key: &str| {
        vars.iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| OsString::from(v))
    };
    sccache_client_side(&var, workspace, files)
}

pub(crate) fn refused(verdict: std::result::Result<(), String>, needle: &str) {
    let why = verdict.expect_err("expected a refusal");
    assert!(why.contains(needle), "{why:?} does not mention {needle:?}");
}

pub(crate) fn workspace_with_sibling() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let ws = root.path().join("ws");
    let outside = root.path().join("outside");
    std::fs::create_dir_all(&ws).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), "keep\n").unwrap();
    (root, ws, outside)
}

pub(crate) fn sentinel(outside: &Path) -> Option<String> {
    std::fs::read_to_string(outside.join("sentinel")).ok()
}

pub(crate) fn snapshot(root: &Path) -> Vec<(PathBuf, String)> {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let m = std::fs::symlink_metadata(&path).unwrap();
            let what = if m.file_type().is_symlink() {
                format!("link -> {}", std::fs::read_link(&path).unwrap().display())
            } else if m.is_dir() {
                stack.push(path.clone());
                #[cfg(unix)]
                {
                    format!("dir {:o}", m.mode())
                }
                #[cfg(not(unix))]
                {
                    "dir".to_string()
                }
            } else {
                #[cfg(unix)]
                {
                    format!(
                        "file {:o} mtime={}.{} {:?}",
                        m.mode(),
                        m.mtime(),
                        m.mtime_nsec(),
                        String::from_utf8_lossy(&std::fs::read(&path).unwrap())
                    )
                }
                #[cfg(not(unix))]
                {
                    format!(
                        "file {:?}",
                        String::from_utf8_lossy(&std::fs::read(&path).unwrap())
                    )
                }
            };
            out.push((path.strip_prefix(root).unwrap().to_path_buf(), what));
        }
    }
    out.sort();
    out
}

pub(crate) async fn in_both_modes(
    make: impl Fn() -> Job,
    check: impl Fn(&str, ShadowHypothesisResult),
) {
    let (_tx, rx) = tokio::sync::watch::channel(false);
    match overlay_unavailable() {
        None => check("overlay", run_overlay(make(), rx.clone()).await),
        Some(reason) => eprintln!("overlay mode skipped: {reason}"),
    }
    check("in-place", run_in_place(make(), rx).await);
}

pub(crate) fn refused_with(mode: &str, r: &ShadowHypothesisResult, needle: &str) {
    assert!(
        r.exit_code.is_none() && r.output_tail.is_none(),
        "{mode}: the command ran: {:?} {}",
        r.error,
        output(r)
    );
    let why = r.error.as_deref().unwrap_or_default();
    assert!(
        why.contains(needle),
        "{mode}: {why:?} does not mention {needle:?}"
    );
}

pub(crate) fn injected(what: &str) -> std::io::Error {
    std::io::Error::other(format!("injected: {what}"))
}

pub(crate) fn in_place_lock_users(workspace: &Path) -> Option<usize> {
    let locks = in_place_locks().lock().unwrap();
    locks.get(&in_place_key(workspace)).map(Weak::strong_count)
}

pub(crate) async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
