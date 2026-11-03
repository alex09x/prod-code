/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! The bounded `npm root -g` probe and its private log directory (#1794).

use std::ffi::OsString;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::process::OwnedProcessGroup;

/// How long an npm root discovery probe gets before it is aborted and reaped (#1794).
pub const DEFAULT_NPM_PROBE_TIMEOUT: Duration = Duration::from_secs(4);

/// Executes `npm root -g` without persistent logging, bounded by `timeout` (#1794).
///
/// npm cleans its log directory even with `logs-max=0` (it lists `logs-dir`, by default
/// `<cache>/_logs`, and deletes old `*-debug-*.log` files). So the probe sets `logs-dir`
/// on the command line and in the environment to a not yet existing child of a fresh
/// directory private to this process, which is removed afterwards; the user's log
/// directory is never named. When that directory cannot be made, npm is not run.
///
/// The child leads its own process group and is waited for under one deadline; see
/// [`OwnedProcessGroup`] for what is terminated on timeout.
pub fn probe_npm_root(
    npm_exe: &Path,
    timeout: Duration,
    extra_env: impl IntoIterator<Item = (OsString, OsString)>,
) -> Option<PathBuf> {
    let deadline = Instant::now() + timeout;
    let log_dir = ProbeLogDir::create().ok()?;
    let logs = log_dir.logs_dir();
    let mut logs_arg = OsString::from("--logs-dir=");
    logs_arg.push(logs.as_os_str());

    let mut cmd = Command::new(npm_exe);
    cmd.args(["root", "-g", "--logs-max=0", "--no-update-notifier"])
        .arg(&logs_arg)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // After the caller's variables, so nothing can point npm back at the user's logs.
    cmd.env("npm_config_logs_dir", &logs)
        .env("npm_config_logs_max", "0")
        .env("npm_config_update_notifier", "false")
        .env("npm_config_audit", "false");

    let group = match OwnedProcessGroup::spawn(&mut cmd) {
        Ok(group) => group,
        // One retry for a transient failure such as ETXTBSY while npm is being replaced.
        Err(_) if Instant::now() < deadline => {
            std::thread::sleep(Duration::from_millis(15).min(deadline - Instant::now()));
            OwnedProcessGroup::spawn(&mut cmd).ok()?
        }
        Err(_) => return None,
    };
    let output = group.wait_with_deadline(deadline)?;
    drop(log_dir);
    parse_npm_root_output(&output)
}

/// Parses the output of `npm root -g`, ensuring exit success and existing directory.
pub fn parse_npm_root_output(output: &std::process::Output) -> Option<PathBuf> {
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let direct_path = PathBuf::from(trimmed);
    if direct_path.is_dir() {
        return Some(direct_path);
    }
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|p| p.is_dir())
}

/// A fresh directory under the temp dir, created by this process with mode `0700`,
/// that holds the probe's log target and is removed on drop.
pub struct ProbeLogDir {
    root: PathBuf,
}

impl ProbeLogDir {
    /// Creates the directory exclusively under an unpredictable name and checks that
    /// it is a real directory owned by this user and closed to everyone else.
    pub fn create() -> std::io::Result<Self> {
        Self::create_in(&std::env::temp_dir())
    }

    /// [`Self::create`] under `base`.
    pub fn create_in(base: &Path) -> std::io::Result<Self> {
        let state = std::collections::hash_map::RandomState::new();
        let mut last = None;
        for attempt in 0u64..16 {
            let mut hasher = state.build_hasher();
            hasher.write_u64(attempt);
            hasher.write_u32(std::process::id());
            hasher.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_nanos()),
            );
            let root = base.join(format!("prod-code-npm-probe-{:016x}", hasher.finish()));
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(&root) {
                Ok(()) => {
                    // From here on the directory is ours, so drop removes it.
                    let dir = Self { root };
                    dir.check_private()?;
                    return Ok(dir);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| std::io::Error::other("no probe directory name left")))
    }

    /// The log directory handed to npm: a child of the private directory that does
    /// not exist, so there is nothing for npm's cleanup to list.
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    fn check_private(&self) -> std::io::Result<()> {
        let meta = std::fs::symlink_metadata(&self.root)?;
        if !meta.file_type().is_dir() {
            return Err(std::io::Error::other("probe directory is not a directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            unsafe extern "C" {
                fn geteuid() -> u32;
            }
            // SAFETY: geteuid has no preconditions and cannot fail.
            let euid = unsafe { geteuid() };
            if meta.uid() != euid || meta.mode() & 0o077 != 0 {
                return Err(std::io::Error::other("probe directory is not private"));
            }
        }
        Ok(())
    }
}

impl Drop for ProbeLogDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
