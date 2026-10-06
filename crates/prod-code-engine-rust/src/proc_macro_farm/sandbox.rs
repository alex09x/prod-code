/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

/// Ensures a directory exists with private 0700 permissions and owned by the current user.
pub fn ensure_secure_farm_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

        let uid = current_uid();
        match std::fs::symlink_metadata(dir) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    anyhow::bail!(
                        "Security violation: farm directory {} is a symlink",
                        dir.display()
                    );
                }
                if !meta.is_dir() {
                    anyhow::bail!(
                        "Security violation: farm directory path {} is not a directory",
                        dir.display()
                    );
                }
                if meta.uid() != uid {
                    anyhow::bail!(
                        "Security violation: farm directory {} is owned by uid {}, expected uid {}",
                        dir.display(),
                        meta.uid(),
                        uid
                    );
                }
                let mode = meta.mode() & 0o777;
                if mode != 0o700 {
                    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                        .with_context(|| {
                            format!("Failed to set 0700 permissions on {}", dir.display())
                        })?;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(dir)
                    .with_context(|| {
                        format!("Failed to create private farm directory {}", dir.display())
                    })?;
            }
            Err(err) => return Err(err).context("Failed to inspect farm directory metadata"),
        }
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

/// Prepares an isolated, sandboxed proc-macro server executable launcher.
///
/// The returned executable path can be passed directly as
/// `ProcMacroServerChoice::Explicit(path)` to rust-analyzer's `LoadCargoConfig`.
///
/// When invoked by rust-analyzer, the launcher sets up OS-level memory limits,
/// core dump suppression, CPU scheduling de-prioritization, scrubs secrets
/// from the child environment, redirects temp files to an isolated scratch dir,
/// and executes the real sysroot `rust-analyzer-proc-macro-srv`.
pub fn prepare_sandboxed_srv(sysroot_srv: &Path, memory_limit_mb: u64) -> Result<PathBuf> {
    if memory_limit_mb < 64 {
        anyhow::bail!("Invalid proc-macro memory limit {memory_limit_mb}MB: must be at least 64MB");
    }
    if memory_limit_mb > 16 * 1024 * 1024 {
        anyhow::bail!("Invalid proc-macro memory limit {memory_limit_mb}MB: exceeds 16TB maximum");
    }
    let memory_limit_kb = memory_limit_mb.checked_mul(1024).ok_or_else(|| {
        anyhow::anyhow!("Memory limit {memory_limit_mb}MB overflows KB representation")
    })?;

    #[cfg(unix)]
    let uid = current_uid();
    #[cfg(not(unix))]
    let uid = 0;

    let farm_dir = std::env::temp_dir().join(format!("prod-code-proc-macro-farm-{uid}"));
    ensure_secure_farm_dir(&farm_dir).with_context(|| {
        format!(
            "Failed to ensure secure farm directory {}",
            farm_dir.display()
        )
    })?;

    let scratch_dir = farm_dir.join("scratch");
    ensure_secure_farm_dir(&scratch_dir).with_context(|| {
        format!(
            "Failed to ensure secure scratch directory {}",
            scratch_dir.display()
        )
    })?;

    #[cfg(unix)]
    let script = format!(
        r#"#!/bin/sh
# prod-code isolated proc-macro worker farm sandbox
set -e

# Disable core dumps
ulimit -c 0 2>/dev/null || true

# Limit virtual memory / address space in KB (fail closed if limit cannot be enforced)
if ! ulimit -v {memory_limit_kb} 2>/dev/null; then
    echo "FATAL: prod-code sandbox failed to enforce virtual memory limit ({memory_limit_kb} KB)" >&2
    exit 125
fi

# Limit open file descriptors
ulimit -n 2048 2>/dev/null || true

# Dynamically scrub all PROD_CODE_* environment variables to prevent token leakage
for _var in $(env | sed -n 's/^\(PROD_CODE_[^=]*\)=.*/\1/p'); do
    unset "$_var" 2>/dev/null || true
done

# Comprehensively unset known sensitive gateway tokens, keys, and credentials
unset PROD_CODE_AUTH_TOKEN PROD_CODE_AUTH_TOKEN_FILE
unset PROD_CODE_TOKEN PROD_CODE_SECRET PROD_CODE_PASSWORD
unset PROD_CODE_TLS_KEY PROD_CODE_TLS_CERT PROD_CODE_TLS_CA PROD_CODE_TLS_PIN PROD_CODE_TLS_SERVER_NAME
unset PROD_CODE_CERT PROD_CODE_KEY PROD_CODE_CA

# Scrub cloud provider and VCS credentials
unset AWS_ACCESS_KEY_ID AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN AWS_SECURITY_TOKEN AWS_PROFILE AWS_CONFIG_FILE
unset GITHUB_TOKEN GH_TOKEN GITLAB_TOKEN BITBUCKET_TOKEN
unset SSH_AUTH_SOCK SSH_AGENT_PID
unset DATABASE_URL REDIS_URL
unset GCP_TOKEN GOOGLE_APPLICATION_CREDENTIALS

# Set isolated temporary scratch directory
export TMPDIR="{scratch_dir}"
export TEMP="{scratch_dir}"
export TMP="{scratch_dir}"

# Ensure internal proc-macro server protocol authorization
export RUST_ANALYZER_INTERNALS_DO_NOT_USE="this is unstable"

# Lower scheduling priority so macro expansion does not starve LSP queries
if command -v nice >/dev/null 2>&1; then
    exec nice -n 10 "{sysroot_srv}" "$@"
else
    exec "{sysroot_srv}" "$@"
fi
"#,
        memory_limit_kb = memory_limit_kb,
        scratch_dir = scratch_dir.display(),
        sysroot_srv = sysroot_srv.display(),
    );

    #[cfg(not(unix))]
    let script = format!(
        r#"@echo off
set PROD_CODE_AUTH_TOKEN=
set PROD_CODE_AUTH_TOKEN_FILE=
set PROD_CODE_TOKEN=
set PROD_CODE_SECRET=
set PROD_CODE_TLS_KEY=
set PROD_CODE_TLS_CERT=
set AWS_ACCESS_KEY_ID=
set AWS_SECRET_ACCESS_KEY=
set GITHUB_TOKEN=
set TMPDIR={scratch_dir}
set TEMP={scratch_dir}
set TMP={scratch_dir}
set RUST_ANALYZER_INTERNALS_DO_NOT_USE=this is unstable
"{sysroot_srv}" %*
"#,
        scratch_dir = scratch_dir.display(),
        sysroot_srv = sysroot_srv.display(),
    );

    let mut hasher = DefaultHasher::new();
    sysroot_srv.hash(&mut hasher);
    memory_limit_mb.hash(&mut hasher);
    script.hash(&mut hasher);
    let hash = hasher.finish();

    static INSTALL_LOCK: Mutex<()> = Mutex::new(());
    static ATTEMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

    let _install_guard = INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let wrapper_path = farm_dir.join(format!("sandboxed-proc-macro-srv-{hash:016x}.sh"));
        if wrapper_path.exists()
            && let Ok(meta) = std::fs::symlink_metadata(&wrapper_path)
            && meta.is_file()
            && !meta.file_type().is_symlink()
            && meta.uid() == uid
            && (meta.mode() & 0o777) == 0o700
            && std::fs::read_to_string(&wrapper_path).map_or(false, |content| content == script)
        {
            return Ok(wrapper_path);
        }

        // Safe atomic launcher creation: use unique attempt counter to avoid collisions between concurrent attempts
        let attempt = ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_wrapper = farm_dir.join(format!(
            ".tmp-sandboxed-srv-{hash:016x}-{}-{attempt}.sh",
            std::process::id()
        ));

        std::fs::write(&tmp_wrapper, &script).with_context(|| {
            format!("Failed to write sandbox wrapper {}", tmp_wrapper.display())
        })?;

        let mut perms = std::fs::metadata(&tmp_wrapper)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&tmp_wrapper, perms)?;

        std::fs::rename(&tmp_wrapper, &wrapper_path).with_context(|| {
            format!(
                "Failed to atomically install launcher to {}",
                wrapper_path.display()
            )
        })?;

        // Verify installed wrapper metadata
        let meta = std::fs::symlink_metadata(&wrapper_path)?;
        if meta.file_type().is_symlink() || !meta.is_file() || meta.uid() != uid {
            let _ = std::fs::remove_file(&wrapper_path);
            anyhow::bail!(
                "Security violation: installed wrapper {} has invalid metadata or ownership",
                wrapper_path.display()
            );
        }

        Ok(wrapper_path)
    }

    #[cfg(not(unix))]
    {
        let wrapper_path = farm_dir.join(format!("sandboxed-proc-macro-srv-{hash:016x}.cmd"));
        if wrapper_path.exists()
            && std::fs::read_to_string(&wrapper_path).map_or(false, |content| content == script)
        {
            return Ok(wrapper_path);
        }

        let attempt = ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_wrapper = farm_dir.join(format!(".tmp-{hash:016x}-{attempt}.cmd"));
        std::fs::write(&tmp_wrapper, &script)?;
        std::fs::rename(&tmp_wrapper, &wrapper_path)?;
        Ok(wrapper_path)
    }
}
