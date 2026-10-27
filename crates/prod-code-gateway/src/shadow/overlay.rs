/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::Result;
use prod_code_protocol::ShadowHypothesisResult;

use super::process::run_child;
use super::root::{remove_shadow_dir, setpriv_path};
use super::sccache::{check_isolation_conflicts, ensure_sccache_server};
use super::staging::{check_files, dir_name, failed, stage_upper};
use super::types::{Job, RUN_SCRIPT};

/// Runs one hypothesis as an overlay shadow: its files go to a fresh upper directory, the
/// command runs in a user + mount namespace where the workspace path is the overlay.
pub async fn run_overlay(
    job: Job,
    cancel: tokio::sync::watch::Receiver<bool>,
) -> ShadowHypothesisResult {
    if let Err(why) = check_files(&job.workspace, &job.files) {
        return failed(&job.name, format!("invalid hypothesis: {why}"));
    }
    if let Some(err) = check_isolation_conflicts(&job) {
        tracing::info!(hypothesis = %job.name, reason = %err, "🌓 [SHADOW] hypothesis refused");
        return failed(&job.name, err);
    }
    let setpriv = match setpriv_path() {
        Ok(path) => path,
        Err(why) => return failed(&job.name, format!("cannot run an overlay shadow: {why}")),
    };
    let dir = job
        .shadow_root
        .join(dir_name(&job.workspace, &job.name, job.nonce));
    let upper = dir.join("upper");
    let work = dir.join("work");
    let delete_list = dir.join("delete.txt");
    let (control_dir, mount_status) = if job.fallback_shadow_root.is_some() {
        let cd = dir.join("control");
        let ms = cd.join("mount_status");
        (Some(cd), Some(ms))
    } else {
        (None, None)
    };
    let script = dir.join("run.sh");
    // Taken exclusively: a directory already there belongs to another hypothesis, and is
    // neither staged into nor removed (#1466). Other failures go to the staging below, which
    // falls back to the disk root.
    if let Some(parent) = dir.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::create_dir(&dir)
        && e.kind() == std::io::ErrorKind::AlreadyExists
    {
        return failed(
            &job.name,
            format!(
                "cannot stage hypothesis: its directory {} is already in use",
                dir.display()
            ),
        );
    }
    let staged = (|| -> Result<()> {
        std::fs::create_dir_all(&upper)?;
        std::fs::create_dir_all(&work)?;
        if let Some(cd) = &control_dir {
            std::fs::create_dir_all(cd)?;
        }
        let deleted = stage_upper(&upper, &job.files)?;
        let mut list = String::new();
        for path in deleted {
            list.push_str(&path.to_string_lossy());
            list.push('\n');
        }
        std::fs::write(&delete_list, list)?;
        std::fs::write(&script, RUN_SCRIPT)?;
        Ok(())
    })();
    if let Err(e) = staged {
        remove_shadow_dir(&dir);
        if let Some(fallback_root) = job.fallback_shadow_root {
            tracing::warn!(
                hypothesis = %job.name,
                why = %e,
                "🌓 [SHADOW] RAM overlay staging failed; falling back to disk overlay"
            );
            if *cancel.borrow() {
                return failed(&job.name, "cancelled: the client left".to_string());
            }
            if let Some(ran_in_ram) = &job.ran_in_ram {
                ran_in_ram.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            let fallback_job = Job {
                shadow_root: fallback_root,
                fallback_shadow_root: None,
                ..job
            };
            return Box::pin(run_overlay(fallback_job, cancel)).await;
        }
        return failed(&job.name, format!("cannot stage hypothesis: {e:#}"));
    }
    tokio::task::spawn_blocking(ensure_sccache_server)
        .await
        .ok();
    let argv = match command_after_drop(&job.env, &job.argv) {
        Ok(argv) => argv,
        Err(why) => return failed(&job.name, why),
    };
    let mut cmd = tokio::process::Command::new("unshare");
    cmd.args(["-Urm", "--propagation", "private", "sh"])
        .arg(&script)
        .args(&argv)
        // Ensure sccache compiles in the client process inside the mount namespace (#426);
        // a request may override it, and `check_isolation_conflicts` has judged the result.
        .env("SCCACHE_CLIENT_SIDE", "1")
        // The request's env is not here: `unshare`, the run script and `setpriv` keep the
        // gateway's, and `command_after_drop` gives it to the command alone (#1468).
        .env("SHADOW_LOWER", &job.workspace)
        .env("SHADOW_UPPER", &upper)
        .env("SHADOW_WORK", &work)
        .env("SHADOW_DELETE", &delete_list)
        .env("SHADOW_SUBDIR", &job.subdir)
        .env("SHADOW_SETPRIV", setpriv)
        .current_dir(&job.workspace);
    if let (Some(cd), Some(ms)) = (&control_dir, &mount_status) {
        cmd.env("SHADOW_CONTROL_DIR", cd);
        cmd.env("SHADOW_MOUNT_STATUS", ms);
    }
    let result = run_child(cmd, &job, cancel.clone()).await;
    let fallback_root = job.fallback_shadow_root.clone();
    let mount_succeeded = if fallback_root.is_some() {
        mount_status
            .as_ref()
            .map(|p| {
                p.is_file()
                    && std::fs::read_to_string(p)
                        .map(|s| s == "ok")
                        .unwrap_or(false)
            })
            .unwrap_or(false)
    } else {
        true
    };
    // The upper directory can hold a whole incremental build; remove it off the response path.
    tokio::task::spawn_blocking(move || remove_shadow_dir(&dir));
    if !mount_succeeded {
        if let Some(fallback_root) = fallback_root {
            tracing::warn!(
                hypothesis = %job.name,
                "🌓 [SHADOW] RAM overlay mount failed; falling back to disk overlay"
            );
            if *cancel.borrow() {
                return failed(&job.name, "cancelled: the client left".to_string());
            }
            if let Some(ran_in_ram) = &job.ran_in_ram {
                ran_in_ram.store(false, std::sync::atomic::Ordering::Relaxed);
            }
            let fallback_job = Job {
                shadow_root: fallback_root,
                fallback_shadow_root: None,
                ..job
            };
            return Box::pin(run_overlay(fallback_job, cancel)).await;
        }
    }
    result
}

/// The argv `setpriv` runs once it has dropped the capabilities: the request's command, behind
/// `env NAME=VALUE…` when the request sets variables. Given to `unshare` instead, a loader
/// variable (`LD_PRELOAD`) or `PATH` would act on the shell and `setpriv` while they still
/// hold the namespace's capabilities (#1468).
pub(crate) fn command_after_drop(
    env: &[(String, String)],
    argv: &[String],
) -> Result<Vec<String>, String> {
    // Without a command, `env` would print the variables and succeed.
    if env.is_empty() || argv.is_empty() {
        return Ok(argv.to_vec());
    }
    if let Some((name, _)) = env
        .iter()
        .find(|(name, _)| name.is_empty() || name.contains(['=', '\0']))
    {
        return Err(format!("invalid environment variable name {name:?}"));
    }
    // `env` takes every leading `NAME=VALUE` as a variable, so a command spelled like one would
    // be set instead of run.
    if argv.first().is_some_and(|program| program.contains('=')) {
        return Err(format!(
            "the command {:?} has an `=` in its name; run it by a path without one",
            argv[0]
        ));
    }
    let mut out = vec!["env".to_string(), "--".to_string()];
    out.extend(env.iter().map(|(name, value)| format!("{name}={value}")));
    out.extend(argv.iter().cloned());
    Ok(out)
}

/// Whether a hypothesis ran and left nothing to report: exit 0, in time, and (in place) the
/// workspace put back.
pub(crate) fn clean(result: &ShadowHypothesisResult) -> bool {
    result.exit_code == Some(0) && !result.timed_out && result.error.is_none()
}
