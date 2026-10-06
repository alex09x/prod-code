/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use anyhow::{Context, Result, bail};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::types::{HYPOTHESIS_DIR_PREFIX, OWNERSHIP_LOCK_FILE};

/// A storage-specific shadow namespace next to the workspace storage directory. Canonical
/// aliases of one storage directory choose the same namespace, while sibling storage roots do
/// not share one.
pub fn default_root(storage_root: &Path) -> PathBuf {
    if let Ok(custom_root) = std::env::var("PROD_CODE_SHADOW_ROOT")
        && !custom_root.trim().is_empty()
    {
        return PathBuf::from(custom_root.trim());
    }
    let identity = storage_identity(storage_root);
    let label: String = identity
        .file_name()
        .unwrap_or_else(|| OsStr::new("storage"))
        .to_string_lossy()
        .chars()
        .take(32)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let hash = identity
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    if let Ok(ram_env) = std::env::var("PROD_CODE_SHADOW_RAM")
        && (ram_env == "1" || ram_env.eq_ignore_ascii_case("true"))
    {
        if let Some(ram_root) = ram_shadow_root(storage_root) {
            return ram_root;
        }
    }
    identity
        .parent()
        .map(|parent| parent.join(format!(".prod-code-shadow-{label}-{hash:016x}")))
        .unwrap_or_else(|| identity.join(format!(".shadow-{hash:016x}")))
}

/// A lightweight in-memory RAM overlay root in `/dev/shm` for speculative shadow execution (Roadmap 7.4).
/// Returns `None` if `/dev/shm` does not exist or is not a directory.
pub fn ram_shadow_root(storage_root: &Path) -> Option<PathBuf> {
    let shm = Path::new("/dev/shm");
    if !shm.is_dir() {
        return None;
    }
    let identity = storage_identity(storage_root);
    let label: String = identity
        .file_name()
        .unwrap_or_else(|| OsStr::new("storage"))
        .to_string_lossy()
        .chars()
        .take(32)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let hash = identity
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    Some(shm.join(format!(".prod-code-shadow-ram-{label}-{hash:016x}")))
}

pub(crate) fn storage_identity(storage_root: &Path) -> PathBuf {
    if let Ok(canonical) = storage_root.canonicalize() {
        return canonical;
    }
    let absolute = if storage_root.is_absolute() {
        storage_root.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(storage_root)
    };
    let mut identity = PathBuf::new();
    let mut unresolved = false;
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => identity.push(prefix.as_os_str()),
            Component::RootDir => identity.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(name) => {
                let candidate = identity.join(name);
                match candidate.canonicalize() {
                    Ok(canonical) => {
                        identity = canonical;
                        unresolved = false;
                    }
                    Err(_) => {
                        identity = candidate;
                        unresolved = true;
                    }
                }
            }
            Component::ParentDir if unresolved => {
                identity.pop();
                if let Ok(canonical) = identity.canonicalize() {
                    identity = canonical;
                    unresolved = false;
                }
            }
            Component::ParentDir => {
                let candidate = identity.join("..");
                match candidate.canonicalize() {
                    Ok(canonical) => identity = canonical,
                    Err(_) => {
                        identity = candidate;
                        unresolved = true;
                    }
                }
            }
        }
    }
    identity
}

/// Exclusive ownership of one shadow namespace. The lock file is never replaced or swept, so
/// canonical aliases contend on the same inode and dropping this guard releases ownership on
/// every normal return or startup error path.
pub struct ShadowRootOwner {
    root: PathBuf,
    _lock: File,
}

impl ShadowRootOwner {
    pub fn acquire(root: &Path) -> Result<Self> {
        std::fs::create_dir_all(root)
            .with_context(|| format!("cannot create shadow root {}", root.display()))?;
        let root = root
            .canonicalize()
            .with_context(|| format!("cannot resolve shadow root {}", root.display()))?;
        let lock_path = root.join(OWNERSHIP_LOCK_FILE);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let lock = options.open(&lock_path).with_context(|| {
            format!("cannot open shadow ownership lock {}", lock_path.display())
        })?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => bail!(
                "shadow root {} is already owned by another gateway; stop that gateway or choose a distinct --shadow-dir / PROD_CODE_SHADOW_DIR",
                root.display()
            ),
            Err(std::fs::TryLockError::Error(err)) => {
                return Err(err).with_context(|| {
                    format!(
                        "cannot lock shadow root ownership at {}",
                        lock_path.display()
                    )
                });
            }
        }
        Ok(Self { root, _lock: lock })
    }

    /// Removes abandoned hypothesis directories from this exclusively owned namespace. Other
    /// entries, including the stable ownership lock, are never touched.
    pub fn sweep(&self) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            if !entry
                .file_name()
                .to_string_lossy()
                .starts_with(HYPOTHESIS_DIR_PREFIX)
                || !entry.file_type().is_ok_and(|kind| kind.is_dir())
            {
                continue;
            }
            remove_shadow_dir(&entry.path());
            removed += 1;
        }
        removed
    }
}

static RAM_SHADOW_OWNERS: OnceLock<
    Mutex<std::collections::HashMap<PathBuf, Arc<ShadowRootOwner>>>,
> = OnceLock::new();

pub(crate) fn acquire_ram_shadow_root(storage_root: &Path) -> Result<Option<PathBuf>> {
    let Some(root) = ram_shadow_root(storage_root) else {
        return Ok(None);
    };
    let owners = RAM_SHADOW_OWNERS.get_or_init(|| Mutex::new(std::collections::HashMap::new()));
    let mut owners = owners
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if owners.contains_key(&root) {
        return Ok(Some(root));
    }
    let owner = Arc::new(ShadowRootOwner::acquire(&root)?);
    let swept = owner.sweep();
    if swept > 0 {
        tracing::info!(root = %root.display(), swept, "removed leftover RAM shadow hypotheses");
    }
    owners.insert(root.clone(), owner);
    Ok(Some(root))
}

/// `None` when this node can run hypotheses as overlay shadows, otherwise why it cannot.
/// Probed once per process: a throw-away overlay mount inside a user namespace.
pub fn overlay_unavailable() -> Option<&'static str> {
    static PROBE: OnceLock<Option<String>> = OnceLock::new();
    PROBE.get_or_init(probe_overlay).as_deref()
}

/// The gateway's trusted `setpriv` executable. The request can replace PATH, so the run script
/// receives this absolute path after request environment variables.
pub(crate) fn setpriv_path() -> std::result::Result<PathBuf, String> {
    let path = std::env::var_os("PATH").ok_or_else(|| {
        "cannot find setpriv: the gateway PATH is unset; overlay shadows need setpriv to drop \
         namespace-root capabilities"
            .to_string()
    })?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("setpriv");
        if candidate.is_file() {
            return candidate
                .canonicalize()
                .map_err(|e| format!("cannot resolve {}: {e}", candidate.display()));
        }
    }
    Err(
        "cannot find setpriv on the gateway PATH; overlay shadows need it to drop \
         namespace-root capabilities"
            .to_string(),
    )
}

fn probe_overlay() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return Some("overlay shadows need Linux (user namespaces + overlayfs)".to_string());
    }
    let setpriv = match setpriv_path() {
        Ok(path) => path,
        Err(why) => return Some(why),
    };
    let dir = std::env::temp_dir().join(format!("prod-code-shadow-probe-{}", std::process::id()));
    let (lower, upper, work, mnt) = (
        dir.join("lower"),
        dir.join("upper"),
        dir.join("work"),
        dir.join("mnt"),
    );
    let prepared = (|| -> std::io::Result<()> {
        for d in [&lower, &upper, &work, &mnt] {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(lower.join("probe"), "ok\n")
    })();
    if let Err(e) = prepared {
        return Some(format!("cannot prepare {}: {e}", dir.display()));
    }
    let script = format!(
        r#"mount -t overlay overlay -o lowerdir={l},upperdir={u},workdir={w} {m} || exit
chmod 0444 {m}/probe || exit
if ! "$SHADOW_SETPRIV" --bounding-set=-all --inh-caps=-all --ambient-caps=-all --no-new-privs \
    sh -c 'printf changed > "$1/probe" 2>/dev/null; test $? -ne 0' sh {m}; then
    echo "setpriv did not drop namespace-root file-write privileges" >&2
    exit 1
fi
test "$(cat {m}/probe)" = ok && printf 'ok\n'"#,
        l = lower.display(),
        u = upper.display(),
        w = work.display(),
        m = mnt.display()
    );
    let output = std::process::Command::new("unshare")
        .args(["-Urm", "--propagation", "private", "sh", "-c", &script])
        .env("SHADOW_SETPRIV", setpriv)
        .output();
    let verdict = match output {
        Ok(o) if o.status.success() && o.stdout == b"ok\n" => None,
        Ok(o) => Some(format!(
            "unprivileged overlay mount failed: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(e) => Some(format!("cannot run unshare: {e}")),
    };
    remove_shadow_dir(&dir);
    verdict
}

/// Removes a shadow directory. The overlay work directory holds entries created by the
/// namespace's root with mode 000, which the gateway's own uid cannot delete directly, so
/// the removal is retried inside a user namespace.
pub fn remove_shadow_dir(dir: &Path) {
    if !dir.exists() {
        return;
    }
    if std::fs::remove_dir_all(dir).is_ok() {
        return;
    }
    let _ = std::process::Command::new("unshare")
        .args(["-Urm", "rm", "-rf"])
        .arg(dir)
        .status();
    if dir.exists() {
        tracing::warn!(dir = %dir.display(), "could not remove shadow directory");
    }
}
