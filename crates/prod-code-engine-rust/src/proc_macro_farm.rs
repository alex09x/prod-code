//! Isolated procedural macro worker farm and sandboxing for rust-analyzer.
//!
//! Replaces unconstrained, per-workspace `proc_macro_srv` process spawning with:
//! 1. A shared node-level worker farm (`ProcMacroWorkerFarm`) that bounds total
//!    worker processes across all loaded workspaces on the gateway node.
//! 2. OS-level process sandboxing (`ProcMacroSandbox`):
//!    - Virtual address space limits (`RLIMIT_AS` via `ulimit -v`) to prevent
//!      runaway macro expansion or memory leaks from exhausting host RAM.
//!    - Core dump suppression (`RLIMIT_CORE = 0`) to prevent leaking process memory.
//!    - Open file descriptor bounds (`RLIMIT_NOFILE`).
//!    - Nice priority lowering (+10) so macro expansion does not starve LSP queries.
//!    - Sensitive cluster secret and token scrubbing (`unset PROD_CODE_*`, `AWS_*`, `GITHUB_*`, etc.).
//!    - Isolated scratch temporary directory (`TMPDIR`).

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// Metrics snapshot of the shared proc-macro worker farm.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FarmMetrics {
    /// Maximum worker processes allowed across the entire gateway node.
    pub capacity: usize,
    /// Currently allocated worker processes across all active workspaces.
    pub active_workers: usize,
    /// Number of active workspaces holding worker permits.
    pub active_workspaces: usize,
    /// Default virtual address space limit in megabytes for sandboxed workers.
    pub default_memory_limit_mb: u64,
}

/// Node-wide shared proc-macro worker farm managing worker process concurrency.
#[derive(Debug)]
pub struct ProcMacroWorkerFarm {
    /// Global worker process capacity.
    capacity: usize,
    /// Total worker processes currently allocated.
    active_workers: Mutex<usize>,
    /// Active workspace allocations: workspace root -> allocated worker count.
    allocations: Mutex<HashMap<PathBuf, usize>>,
    /// Condition variable to notify waiting workspace allocations when permits are released.
    cvar: Condvar,
}

impl ProcMacroWorkerFarm {
    /// Creates a new worker farm with a specified global worker capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            active_workers: Mutex::new(0),
            allocations: Mutex::new(HashMap::new()),
            cvar: Condvar::new(),
        }
    }

    /// Global worker capacity across all workspaces.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Total worker processes currently allocated across all active workspaces.
    pub fn active_workers(&self) -> usize {
        *self.active_workers.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Number of active workspaces holding worker permits.
    pub fn active_workspaces(&self) -> usize {
        self.allocations.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Returns a telemetry snapshot of the worker farm.
    pub fn metrics(&self) -> FarmMetrics {
        FarmMetrics {
            capacity: self.capacity(),
            active_workers: self.active_workers(),
            active_workspaces: self.active_workspaces(),
            default_memory_limit_mb: 2048,
        }
    }

    /// Allocates worker processes for a workspace from the shared farm immediately.
    ///
    /// The allocated count is bounded by the global farm capacity and remaining
    /// available slots. If the farm is at full capacity, 0 workers are allocated
    /// to strictly uphold the node-level capacity guarantee without overcommitting.
    pub fn allocate_workers(
        self: &Arc<Self>,
        workspace_root: &Path,
        desired: usize,
    ) -> (usize, ProcMacroFarmPermit) {
        self.allocate_workers_timeout(workspace_root, desired, Duration::ZERO)
    }

    /// Allocates worker processes for a workspace, optionally waiting up to `timeout`
    /// for capacity to become available if the farm is currently full.
    pub fn allocate_workers_timeout(
        self: &Arc<Self>,
        workspace_root: &Path,
        desired: usize,
        timeout: Duration,
    ) -> (usize, ProcMacroFarmPermit) {
        if desired == 0 {
            return (
                0,
                ProcMacroFarmPermit {
                    farm: Arc::clone(self),
                    workspace: workspace_root.to_path_buf(),
                    count: 0,
                },
            );
        }

        let mut active = self.active_workers.lock().unwrap_or_else(|e| e.into_inner());
        if *active >= self.capacity && !timeout.is_zero() {
            match self.cvar.wait_timeout_while(active, timeout, |act| *act >= self.capacity) {
                Ok((new_active, _)) => {
                    active = new_active;
                }
                Err(e) => {
                    active = e.into_inner().0;
                }
            }
        }

        let remaining = self.capacity.saturating_sub(*active);
        let allocated = desired.min(remaining);
        if allocated == 0 {
            tracing::warn!(
                workspace = %workspace_root.display(),
                active_workers = *active,
                farm_capacity = self.capacity,
                "Proc-macro farm is at capacity; rejecting worker allocation to prevent overcommit"
            );
            return (
                0,
                ProcMacroFarmPermit {
                    farm: Arc::clone(self),
                    workspace: workspace_root.to_path_buf(),
                    count: 0,
                },
            );
        }

        let mut allocs = self.allocations.lock().unwrap_or_else(|e| e.into_inner());
        *active += allocated;
        *allocs.entry(workspace_root.to_path_buf()).or_insert(0) += allocated;

        tracing::info!(
            workspace = %workspace_root.display(),
            allocated_workers = allocated,
            total_active_workers = *active,
            farm_capacity = self.capacity,
            "Allocated proc-macro farm workers"
        );

        (
            allocated,
            ProcMacroFarmPermit {
                farm: Arc::clone(self),
                workspace: workspace_root.to_path_buf(),
                count: allocated,
            },
        )
    }

    fn release(&self, workspace: &Path, count: usize) {
        if count == 0 {
            return;
        }
        let mut active = self.active_workers.lock().unwrap_or_else(|e| e.into_inner());
        let mut allocs = self.allocations.lock().unwrap_or_else(|e| e.into_inner());

        *active = active.saturating_sub(count);
        if let Some(current) = allocs.get_mut(workspace) {
            *current = current.saturating_sub(count);
            if *current == 0 {
                allocs.remove(workspace);
            }
        }
        drop(allocs);
        drop(active);
        self.cvar.notify_all();
    }

    /// Access the shared node-level process-global farm singleton.
    pub fn shared() -> &'static Arc<ProcMacroWorkerFarm> {
        static FARM: OnceLock<Arc<ProcMacroWorkerFarm>> = OnceLock::new();
        FARM.get_or_init(default_node_farm)
    }
}

/// Access the shared node-level process-global farm singleton.
pub fn shared() -> &'static Arc<ProcMacroWorkerFarm> {
    ProcMacroWorkerFarm::shared()
}

/// RAII permit for worker processes allocated to an active workspace.
///
/// When the `RustEngine` or workspace is dropped or evicted from LRU cache,
/// the permit's `Drop` implementation automatically returns the allocated
/// worker process quota back to the shared `ProcMacroWorkerFarm`.
#[derive(Debug)]
pub struct ProcMacroFarmPermit {
    farm: Arc<ProcMacroWorkerFarm>,
    workspace: PathBuf,
    count: usize,
}

impl ProcMacroFarmPermit {
    /// Number of worker processes held by this permit.
    pub fn worker_count(&self) -> usize {
        self.count
    }

    /// Associated workspace root path.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }
}

impl Drop for ProcMacroFarmPermit {
    fn drop(&mut self) {
        self.farm.release(&self.workspace, self.count);
    }
}

/// Computes the default node-level farm capacity based on available CPU cores.
fn default_node_farm() -> Arc<ProcMacroWorkerFarm> {
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);

    let default_capacity = if let Ok(val) = std::env::var("PROD_CODE_PROC_MACRO_WORKERS") {
        val.parse::<usize>().unwrap_or_else(|_| (cpus / 2).clamp(2, 16))
    } else {
        (cpus / 2).clamp(2, 16)
    };

    tracing::info!(
        capacity = default_capacity,
        cpus = cpus,
        "Initialized shared proc-macro worker farm"
    );

    Arc::new(ProcMacroWorkerFarm::new(default_capacity))
}

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
                        .with_context(|| format!("Failed to set 0700 permissions on {}", dir.display()))?;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .recursive(true)
                    .create(dir)
                    .with_context(|| format!("Failed to create private farm directory {}", dir.display()))?;
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
pub fn prepare_sandboxed_srv(
    sysroot_srv: &Path,
    memory_limit_mb: u64,
) -> Result<PathBuf> {
    #[cfg(unix)]
    let uid = current_uid();
    #[cfg(not(unix))]
    let uid = 0;

    let farm_dir = std::env::temp_dir().join(format!("prod-code-proc-macro-farm-{uid}"));
    ensure_secure_farm_dir(&farm_dir)
        .with_context(|| format!("Failed to ensure secure farm directory {}", farm_dir.display()))?;

    let scratch_dir = farm_dir.join("scratch");
    ensure_secure_farm_dir(&scratch_dir)
        .with_context(|| format!("Failed to ensure secure scratch directory {}", scratch_dir.display()))?;

    let mut hasher = DefaultHasher::new();
    sysroot_srv.hash(&mut hasher);
    memory_limit_mb.hash(&mut hasher);
    let hash = hasher.finish();

    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let wrapper_path = farm_dir.join(format!("sandboxed-proc-macro-srv-{hash:016x}.sh"));

        let memory_limit_kb = memory_limit_mb * 1024;
        let script = format!(
            r#"#!/bin/sh
# prod-code isolated proc-macro worker farm sandbox
set -e

# Disable core dumps
ulimit -c 0 2>/dev/null || true

# Limit virtual memory / address space in KB
ulimit -v {memory_limit_kb} 2>/dev/null || true

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

        // Safe atomic launcher creation: write to a private temporary file and rename
        let tmp_wrapper = farm_dir.join(format!(
            ".tmp-sandboxed-srv-{hash:016x}-{}.sh",
            std::process::id()
        ));

        std::fs::write(&tmp_wrapper, &script)
            .with_context(|| format!("Failed to write sandbox wrapper {}", tmp_wrapper.display()))?;

        let mut perms = std::fs::metadata(&tmp_wrapper)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&tmp_wrapper, perms)?;

        std::fs::rename(&tmp_wrapper, &wrapper_path)
            .with_context(|| format!("Failed to atomically install launcher to {}", wrapper_path.display()))?;

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

        let tmp_wrapper = farm_dir.join(format!(".tmp-{hash:016x}.cmd"));
        std::fs::write(&tmp_wrapper, script)?;
        std::fs::rename(&tmp_wrapper, &wrapper_path)?;
        Ok(wrapper_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_farm_allocation_and_capacity_bounds() {
        let farm = Arc::new(ProcMacroWorkerFarm::new(8));
        assert_eq!(farm.capacity(), 8);
        assert_eq!(farm.active_workers(), 0);
        assert_eq!(farm.active_workspaces(), 0);

        let ws1 = PathBuf::from("/tmp/ws1");
        let (w1, permit1) = farm.allocate_workers(&ws1, 6);
        assert_eq!(w1, 6);
        assert_eq!(permit1.worker_count(), 6);
        assert_eq!(farm.active_workers(), 6);
        assert_eq!(farm.active_workspaces(), 1);

        // Next workspace asks for 6, but only 2 remain in capacity
        let ws2 = PathBuf::from("/tmp/ws2");
        let (w2, permit2) = farm.allocate_workers(&ws2, 6);
        assert_eq!(w2, 2);
        assert_eq!(farm.active_workers(), 8);
        assert_eq!(farm.active_workspaces(), 2);

        // Third workspace asks for workers when capacity is full: strictly rejected without overcommitting
        let ws3 = PathBuf::from("/tmp/ws3");
        let (w3, permit3) = farm.allocate_workers(&ws3, 4);
        assert_eq!(w3, 0, "Must be rejected when farm is at capacity");
        assert_eq!(permit3.worker_count(), 0);
        assert_eq!(farm.active_workers(), 8, "Total active workers must not exceed capacity");
        assert_eq!(farm.active_workspaces(), 2);

        // Dropping ws1 frees 6 workers
        drop(permit1);
        assert_eq!(farm.active_workers(), 2);
        assert_eq!(farm.active_workspaces(), 1);

        // Now ws3 can allocate available capacity
        let (w3_retry, permit3_retry) = farm.allocate_workers(&ws3, 4);
        assert_eq!(w3_retry, 4);
        assert_eq!(farm.active_workers(), 6);
        assert_eq!(farm.active_workspaces(), 2);

        drop(permit2);
        drop(permit3);
        drop(permit3_retry);
        assert_eq!(farm.active_workers(), 0);
        assert_eq!(farm.active_workspaces(), 0);
    }

    #[test]
    fn test_farm_zero_worker_allocation() {
        let farm = Arc::new(ProcMacroWorkerFarm::new(4));
        let ws = PathBuf::from("/tmp/ws_zero");
        let (w, permit) = farm.allocate_workers(&ws, 0);
        assert_eq!(w, 0);
        assert_eq!(permit.worker_count(), 0);
        assert_eq!(farm.active_workers(), 0);
        assert_eq!(farm.active_workspaces(), 0);
        drop(permit);
        assert_eq!(farm.active_workers(), 0);
    }

    #[test]
    fn test_farm_metrics_snapshot() {
        let farm = Arc::new(ProcMacroWorkerFarm::new(12));
        let ws = PathBuf::from("/tmp/ws_metrics");
        let (_w, permit) = farm.allocate_workers(&ws, 4);
        let m = farm.metrics();
        assert_eq!(m.capacity, 12);
        assert_eq!(m.active_workers, 4);
        assert_eq!(m.active_workspaces, 1);
        assert_eq!(m.default_memory_limit_mb, 2048);
        drop(permit);
    }

    #[cfg(unix)]
    #[test]
    fn test_prepare_sandboxed_srv_execution_and_secret_scrubbing() {
        let temp = tempfile::tempdir().unwrap();
        let fake_srv = temp.path().join("fake-proc-macro-srv.sh");
        std::fs::write(
            &fake_srv,
            r#"#!/bin/sh
if [ "$1" = "--version" ]; then
    echo "fake-proc-macro-srv 1.0.0"
    exit 0
fi
echo "AUTH_TOKEN='$PROD_CODE_AUTH_TOKEN'"
echo "AUTH_FILE='$PROD_CODE_AUTH_TOKEN_FILE'"
echo "SECRET_TOKEN='$PROD_CODE_TOKEN'"
echo "INTERNAL='$RUST_ANALYZER_INTERNALS_DO_NOT_USE'"
echo "TMP='$TMPDIR'"
"#,
        )
        .unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&fake_srv).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&fake_srv, perms).unwrap();

        let wrapper = prepare_sandboxed_srv(&fake_srv, 1024).unwrap();
        assert!(wrapper.exists());

        // Test --version invocation
        let out = std::process::Command::new(&wrapper)
            .arg("--version")
            .output()
            .unwrap();
        assert!(out.status.success());
        let version_str = String::from_utf8_lossy(&out.stdout);
        assert!(version_str.contains("fake-proc-macro-srv 1.0.0"));

        // Test secret scrubbing and environment setup
        let out = std::process::Command::new(&wrapper)
            .env("PROD_CODE_AUTH_TOKEN", "prod-auth-token-xyz")
            .env("PROD_CODE_AUTH_TOKEN_FILE", "/tmp/token.secret")
            .env("PROD_CODE_TOKEN", "super-secret-token")
            .output()
            .unwrap();
        assert!(out.status.success());
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("AUTH_TOKEN=''"), "Must scrub PROD_CODE_AUTH_TOKEN: {stdout}");
        assert!(stdout.contains("AUTH_FILE=''"), "Must scrub PROD_CODE_AUTH_TOKEN_FILE: {stdout}");
        assert!(stdout.contains("SECRET_TOKEN=''"), "Must scrub PROD_CODE_TOKEN: {stdout}");
        assert!(stdout.contains("INTERNAL='this is unstable'"), "Must export internal authorization: {stdout}");
        assert!(stdout.contains("/scratch"), "Must set isolated scratch TMPDIR: {stdout}");
    }
}
