//! Cross-worktree Python engine support: shared virtual-environment stub cache,
//! typings indexing, and basedpyright/pyright stubPath coordination (Roadmap 3.6).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use prod_code_protocol::transport::ScrubSecrets;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{DiskSpace, disk_space, seed_fits};

#[allow(dead_code)]
static NEXT_STAGING_ID: AtomicU64 = AtomicU64::new(1);

/// The environment variable name used to explicitly configure the shared Python stub cache directory.
pub const PYTHON_STUB_CACHE_ENV: &str = "PROD_CODE_PYTHON_STUB_CACHE";

/// Returns the path to the shared Python virtual-environment stub cache directory.
///
/// Precedence:
/// 1. `PROD_CODE_PYTHON_STUB_CACHE` environment variable if set.
/// 2. `$HOME/.cache/prod-code/python-stubs`.
/// 3. `/var/tmp/prod-code/python-stubs` (or `/tmp/prod-code/python-stubs`).
/// 4. Temporary directory fallback (`std::env::temp_dir().join("prod-code-python-stubs")`).
///
/// Ensures the directory exists with mode `0700` on Unix systems.
pub fn python_stub_cache_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(PYTHON_STUB_CACHE_ENV) {
        let p = PathBuf::from(custom);
        let _ = ensure_cache_dir(&p);
        return p;
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("python-stubs");
        if ensure_cache_dir(&p).is_ok() {
            return p;
        }
    }

    let var_tmp = PathBuf::from("/var/tmp/prod-code/python-stubs");
    if ensure_cache_dir(&var_tmp).is_ok() {
        return var_tmp;
    }

    let temp = std::env::temp_dir().join("prod-code-python-stubs");
    let _ = ensure_cache_dir(&temp);
    temp
}

/// Ensures `dir` exists and has secure permissions (`0700` on Unix).
pub fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

/// Returns the environment variables that configure the shared Python stub cache
/// across basedpyright, pyright, mypy, and Python type checkers (Roadmap 3.6).
pub fn python_stub_cache_env() -> Vec<(String, String)> {
    let dir = python_stub_cache_dir();
    let dir_str = dir.to_string_lossy().into_owned();
    vec![
        (PYTHON_STUB_CACHE_ENV.to_string(), dir_str.clone()),
        ("MYPYPATH".to_string(), dir_str.clone()),
        ("TYPINGS_PATH".to_string(), dir_str),
    ]
}

/// Checks whether `root` represents or contains a Python project.
pub fn is_python_project(root: &Path) -> bool {
    const PYTHON_MARKERS: &[&str] = &[
        "pyproject.toml",
        "setup.py",
        "setup.cfg",
        "requirements.txt",
        "Pipfile",
        "pyrightconfig.json",
        "mypy.ini",
        ".python-version",
    ];

    for marker in PYTHON_MARKERS {
        if root.join(marker).is_file() {
            return true;
        }
    }

    if root.join(".venv").join("pyvenv.cfg").is_file()
        || root.join("venv").join("pyvenv.cfg").is_file()
        || root.join("pyvenv.cfg").is_file()
    {
        return true;
    }

    // Check for any top-level or immediate subfolder python files
    if let Ok(entries) = fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("py") {
                return true;
            }
        }
    }

    false
}

/// Discovers PEP 561 type stub directories (`*-stubs`) in Python virtual environments.
pub fn find_venv_stubs(venv_root: &Path) -> Vec<PathBuf> {
    let mut stubs = Vec::new();
    let lib_dir = venv_root.join("lib");
    let Ok(entries) = fs::read_dir(&lib_dir) else {
        return stubs;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("python") && path.is_dir() {
            let sp = path.join("site-packages");
            if let Ok(sp_entries) = fs::read_dir(&sp) {
                for sp_entry in sp_entries.flatten() {
                    let sp_path = sp_entry.path();
                    let sp_name = sp_entry.file_name();
                    let sp_name_str = sp_name.to_string_lossy();
                    if sp_name_str.ends_with("-stubs") && sp_path.is_dir() {
                        stubs.push(sp_path);
                    }
                }
            }
        }
    }
    stubs
}

struct TargetLock {
    #[cfg(unix)]
    _file: std::fs::File,
}

impl TargetLock {
    fn acquire(lock_path: &Path) -> io::Result<Self> {
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret != 0 {
                return Err(io::Error::last_os_error());
            }
        }

        Ok(TargetLock {
            #[cfg(unix)]
            _file: file,
        })
    }
}

#[cfg(unix)]
fn sync_mtime(src: &Path, dst: &Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
    if let Ok(meta) = fs::metadata(src) {
        let times = [
            libc::timespec {
                tv_sec: meta.atime() as libc::time_t,
                tv_nsec: meta.atime_nsec() as libc::c_long,
            },
            libc::timespec {
                tv_sec: meta.mtime() as libc::time_t,
                tv_nsec: meta.mtime_nsec() as libc::c_long,
            },
        ];
        if let Ok(c_path) = std::ffi::CString::new(dst.as_os_str().as_bytes()) {
            unsafe {
                libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), 0);
            }
        }
    }
}

#[cfg(not(unix))]
fn sync_mtime(_src: &Path, _dst: &Path) {}

static STUB_FILE_NONCE: AtomicU64 = AtomicU64::new(1);

/// Copies a file to `dst` atomically under a per-target lock, ensuring that concurrent seeders
/// do not race and that older files never overwrite newer files.
fn copy_and_publish_stub(src: &Path, dst: &Path) -> io::Result<u64> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination file has no parent directory")
    })?;
    ensure_cache_dir(parent)?;

    let file_name = dst.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination file has no name")
    })?;
    let lock_path = parent.join(format!(".lock-{}", file_name.to_string_lossy()));
    let _lock = TargetLock::acquire(&lock_path)?;

    // Recheck modification times under the lock to ensure newest-wins policy
    let src_meta = fs::metadata(src)?;
    let src_mod = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    if let Ok(dst_meta) = fs::metadata(dst) {
        let dst_mod = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if dst_mod > src_mod || (dst_mod == src_mod && dst_meta.len() == src_meta.len()) {
            return Ok(0);
        }
    }

    let nonce = STUB_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(".tmp-stub-{pid}-{nonce}-{ts:x}");
    let tmp_path = parent.join(tmp_name);

    let mut src_file = fs::File::open(src)?;
    let mut tmp_file = match fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&tmp_path)
    {
        Ok(f) => f,
        Err(e) => return Err(e),
    };

    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;
        let ret = unsafe { libc::flock(tmp_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if ret != 0 {
            let _ = fs::remove_file(&tmp_path);
            return Err(io::Error::last_os_error());
        }
    }

    let bytes = match io::copy(&mut src_file, &mut tmp_file) {
        Ok(b) => b,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(e);
        }
    };

    if let Err(e) = tmp_file.sync_all() {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = fs::rename(&tmp_path, dst) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    drop(tmp_file);

    sync_mtime(src, dst);

    Ok(bytes)
}

#[derive(Default)]
struct VisitedDirs {
    #[cfg(unix)]
    dev_ino: std::collections::HashSet<(u64, u64)>,
    #[cfg(not(unix))]
    canonical: std::collections::HashSet<PathBuf>,
}

impl VisitedDirs {
    fn insert(&mut self, path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = fs::symlink_metadata(path) {
                if meta.file_type().is_symlink() {
                    return false;
                }
                return self.dev_ino.insert((meta.dev(), meta.ino()));
            }
            false
        }
        #[cfg(not(unix))]
        {
            if let Ok(meta) = fs::symlink_metadata(path) {
                if meta.file_type().is_symlink() {
                    return false;
                }
            }
            if let Ok(canon) = path.canonicalize() {
                return self.canonical.insert(canon);
            }
            false
        }
    }
}

/// Recursively copies/merges type stubs from `src_dir` into `dst_dir`.
///
/// Only `.pyi`, `.typed`, and package directories containing them are indexed.
/// Strictly skips directory symlinks and tracks visited directory inodes to prevent recursion cycles (#835).
/// Returns total bytes written or updated.
pub fn merge_stubs(src_dir: &Path, dst_dir: &Path) -> io::Result<u64> {
    let mut visited = VisitedDirs::default();
    merge_stubs_inner(src_dir, dst_dir, &mut visited)
}

fn merge_stubs_inner(
    src_dir: &Path,
    dst_dir: &Path,
    visited: &mut VisitedDirs,
) -> io::Result<u64> {
    if !src_dir.is_dir() || !visited.insert(src_dir) {
        return Ok(0);
    }
    ensure_cache_dir(dst_dir)?;

    let mut bytes_written = 0u64;
    let Ok(entries) = fs::read_dir(src_dir) else {
        return Ok(0);
    };

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        // Strictly skip symlinks to prevent traversal outside cache or cyclic recursion
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let file_name = entry.file_name();
        let name_str = file_name.to_string_lossy();

        if name_str.starts_with('.') {
            continue;
        }

        let target_path = dst_dir.join(&file_name);

        if file_type.is_dir() {
            bytes_written += merge_stubs_inner(&path, &target_path, visited)?;
        } else if file_type.is_file() {
            let is_stub_file = name_str.ends_with(".pyi")
                || name_str == "py.typed"
                || name_str.ends_with(".py");

            if !is_stub_file {
                continue;
            }

            let should_check = match (fs::metadata(&path), fs::metadata(&target_path)) {
                (Ok(src_meta), Ok(dst_meta)) => {
                    let src_mod = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let dst_mod = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    src_mod > dst_mod || (src_mod == dst_mod && src_meta.len() != dst_meta.len())
                }
                (Ok(_), Err(_)) => true,
                _ => false,
            };

            if should_check {
                let copied = copy_and_publish_stub(&path, &target_path)?;
                bytes_written += copied;
            }
        }
    }

    Ok(bytes_written)
}

/// Seeds Python type stubs and connects the workspace to the shared virtual-environment
/// stub cache for basedpyright, pyright, and mypy (Roadmap 3.6).
pub fn seed_python_worktree(from: &Path, to: &Path) -> io::Result<Option<u64>> {
    seed_python_worktree_within(from, to, disk_space(to))
}

/// Seeds Python type stubs respecting a provided disk space budget.
pub fn seed_python_worktree_within(
    from: &Path,
    to: &Path,
    space: Option<DiskSpace>,
) -> io::Result<Option<u64>> {
    if !is_python_project(from) {
        return Ok(None);
    }

    let cache_dir = python_stub_cache_dir();
    let mut total_bytes = 0u64;

    // 1. Gather all candidate stub sources to calculate aggregate disk requirement
    let from_typings = from.join("typings");
    let typings_size = if from_typings.is_dir() {
        tree_size(&from_typings)
    } else {
        0
    };

    let mut venv_stubs = Vec::new();
    for venv_name in &[".venv", "venv"] {
        let venv_path = from.join(venv_name);
        if venv_path.is_dir() {
            venv_stubs.extend(find_venv_stubs(&venv_path));
        }
    }
    let venv_stubs_size: u64 = venv_stubs.iter().map(|s| tree_size(s)).sum();
    let aggregate_stubs_size = typings_size + venv_stubs_size;

    // Enforce aggregate disk budget across both project typings and virtual environment stubs
    if aggregate_stubs_size > 0 && seed_fits("python type stubs cache", aggregate_stubs_size, space) {
        if typings_size > 0 {
            let merged = merge_stubs(&from_typings, &cache_dir)?;
            total_bytes += merged;
        }

        for stub_dir in venv_stubs {
            let stub_name = stub_dir.file_name().unwrap_or_default();
            let dst_stub = cache_dir.join(stub_name);
            let merged = merge_stubs(&stub_dir, &dst_stub)?;
            total_bytes += merged;
        }
    }

    // 3. Establish `to/typings` symlink pointing to the shared stub cache
    let to_typings = to.join("typings");
    if !to_typings.exists() {
        if let Some(parent) = to_typings.parent() {
            fs::create_dir_all(parent)?;
        }
        #[cfg(unix)]
        {
            if std::os::unix::fs::symlink(&cache_dir, &to_typings).is_ok() {
                total_bytes += 1;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = copy_dir_preserving(&cache_dir, &to_typings);
        }
    } else if fs::symlink_metadata(&to_typings)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(false)
    {
        // Symlink already exists
    } else if to_typings.is_dir() {
        // If it's already a real directory, merge into shared cache
        let merged = merge_stubs(&to_typings, &cache_dir)?;
        total_bytes += merged;
    }

    // 4. Update pyrightconfig.json in `to` if present to include stubPath
    let to_pyright_config = to.join("pyrightconfig.json");
    if to_pyright_config.is_file() {
        if let Ok(content) = fs::read_to_string(&to_pyright_config) {
            if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(obj) = val.as_object_mut() {
                    if !obj.contains_key("stubPath") {
                        obj.insert(
                            "stubPath".to_string(),
                            serde_json::Value::String("typings".to_string()),
                        );
                        if let Ok(updated) = serde_json::to_string_pretty(&val) {
                            let _ = fs::write(&to_pyright_config, updated);
                        }
                    }
                }
            }
        }
    }

    Ok(Some(total_bytes))
}

/// Recursively computes total size of all regular files in a directory.
/// Strictly skips directory symlinks and tracks visited directory inodes to prevent recursion cycles (#835).
pub fn tree_size(dir: &Path) -> u64 {
    let mut visited = VisitedDirs::default();
    tree_size_inner(dir, &mut visited)
}

fn tree_size_inner(dir: &Path, visited: &mut VisitedDirs) -> u64 {
    if !visited.insert(dir) {
        return 0;
    }
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                total += tree_size_inner(&entry.path(), visited);
            } else if file_type.is_file() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

/// Copies a directory tree preserving modification times and symbolic links.
/// Uses exclusive staging holder directory and atomic rename (#834).
#[allow(dead_code)]
fn copy_dir_preserving(src: &Path, dst: &Path) -> io::Result<u64> {
    if dst.exists() {
        return Ok(tree_size(dst));
    }
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination has no parent")
    })?;
    fs::create_dir_all(parent)?;

    let dst_name = dst
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("stubs");

    let mut attempts = 0;
    let (holder, staging_dst) = loop {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            ^ ((NEXT_STAGING_ID.fetch_add(1, Ordering::Relaxed) as u128) << 64)
            ^ ((std::process::id() as u128) << 32);
        let holder_path = parent.join(format!(".staging-holder-{dst_name}-{nonce:032x}"));
        match fs::create_dir(&holder_path) {
            Ok(()) => {
                let staging_path = holder_path.join("content");
                break (holder_path, staging_path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts > 1000 {
                    return Err(io::Error::other(
                        "exhausted attempts to create exclusive staging directory",
                    ));
                }
            }
            Err(e) => return Err(e),
        }
    };

    #[cfg(unix)]
    let copy_result = {
        let mut cmd = std::process::Command::new("cp");
        cmd.arg("-a").arg(src).arg(&staging_dst);
        cmd.scrub_cluster_secrets();
        match cmd.status() {
            Ok(s) if s.success() => Ok(()),
            Ok(s) => Err(io::Error::other(format!(
                "cp -a {} {} failed with status {}",
                src.display(),
                staging_dst.display(),
                s
            ))),
            Err(e) => Err(e),
        }
    };

    #[cfg(not(unix))]
    let copy_result = copy_dir_fallback(src, &staging_dst).map(|_| ());

    if let Err(e) = copy_result {
        let _ = fs::remove_dir_all(&holder);
        return Err(e);
    }

    if let Err(e) = fs::rename(&staging_dst, dst) {
        let _ = fs::remove_dir_all(&holder);
        if dst.exists() {
            return Ok(tree_size(dst));
        }
        return Err(e);
    }

    let _ = fs::remove_dir(&holder);
    Ok(tree_size(dst))
}

#[cfg(not(unix))]
fn copy_dir_fallback(src: &Path, dst: &Path) -> io::Result<u64> {
    fs::create_dir_all(dst)?;
    let mut total = 0;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let target = dst.join(entry.file_name());
        if ft.is_dir() {
            total += copy_dir_fallback(&entry.path(), &target)?;
        } else if ft.is_file() {
            total += fs::copy(&entry.path(), &target)?;
        }
    }
    Ok(total)
}

/// Extracts the creation timestamp from a temporary stub filename formatted as `.tmp-stub-{pid}-{nonce}-{ts:x}`.
pub(crate) fn parse_tmp_stub_timestamp(name: &str) -> Option<SystemTime> {
    let parts: Vec<&str> = name.split('-').collect();
    if parts.len() == 5 && parts[0] == ".tmp" && parts[1] == "stub" {
        let _pid: u32 = parts[2].parse().ok()?;
        let _nonce: u64 = parts[3].parse().ok()?;
        let ts_str = parts[4];
        if let Ok(nanos) = u128::from_str_radix(ts_str, 16) {
            // Must be a reasonable epoch timestamp (after year 2020: ~1.57e18 nanos)
            const MIN_VALID_NANOS: u128 = 1_500_000_000_000_000_000;
            if nanos >= MIN_VALID_NANOS {
                let secs = (nanos / 1_000_000_000) as u64;
                let subsec = (nanos % 1_000_000_000) as u32;
                return Some(SystemTime::UNIX_EPOCH + Duration::new(secs, subsec));
            }
        }
    }
    None
}

/// Grace period for temporary stub files created during atomic copy.
/// Any temporary stub file older than this threshold is considered abandoned by a crashed process
/// and safely unlinked during pruning.
pub const TMP_STUB_GRACE_PERIOD: Duration = Duration::from_secs(3600);

/// Evicts stale stub files from the shared stub cache based on age and max capacity.
/// Uses secure directory-handle-relative traversal and O_NOFOLLOW to eliminate symlink TOCTOU.
pub fn prune_stale_stub_cache(max_age: Duration, max_size_bytes: u64) -> io::Result<usize> {
    let cache_dir = python_stub_cache_dir();
    prune_stale_stub_cache_in(&cache_dir, max_age, max_size_bytes)
}

/// Prunes stale stub cache files within the specified directory using the default 1-hour grace period for temporary files.
pub fn prune_stale_stub_cache_in(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
) -> io::Result<usize> {
    prune_stale_stub_cache_with_grace(cache_dir, max_age, max_size_bytes, TMP_STUB_GRACE_PERIOD)
}

/// Prunes stale stub cache files with a configurable grace period for abandoned temporary stub files.
pub fn prune_stale_stub_cache_with_grace(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    #[cfg(unix)]
    {
        unix_pruner::prune_unix(cache_dir, max_age, max_size_bytes, tmp_grace_period)
    }

    #[cfg(not(unix))]
    {
        prune_fallback(cache_dir, max_age, max_size_bytes, tmp_grace_period)
    }
}

#[cfg(unix)]
mod unix_pruner {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    pub struct CacheEntry {
        pub rel_components: Vec<std::ffi::CString>,
        pub file_name: std::ffi::CString,
        pub size: u64,
        pub modified: SystemTime,
    }

    pub fn prune_unix(
        cache_dir: &Path,
        max_age: Duration,
        max_size_bytes: u64,
        tmp_grace_period: Duration,
    ) -> io::Result<usize> {
        let canonical_root = match fs::canonicalize(cache_dir) {
            Ok(c) => c,
            Err(_) => return Ok(0),
        };
        let expected_meta = match fs::symlink_metadata(&canonical_root) {
            Ok(m) => m,
            Err(_) => return Ok(0),
        };
        if !expected_meta.file_type().is_dir() {
            return Ok(0);
        }

        let c_root = std::ffi::CString::new(canonical_root.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

        let root_fd = unsafe {
            libc::open(
                c_root.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if root_fd < 0 {
            return Ok(0);
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(root_fd, &mut st) } != 0 {
            unsafe { libc::close(root_fd); }
            return Ok(0);
        }

        use std::os::unix::fs::MetadataExt;
        if (st.st_mode & libc::S_IFMT) != libc::S_IFDIR
            || (st.st_dev as u64) != expected_meta.dev()
            || (st.st_ino as u64) != expected_meta.ino()
        {
            unsafe { libc::close(root_fd); }
            return Ok(0);
        }

        let mut files = Vec::new();
        let mut total_size = 0u64;
        let mut rel_components = Vec::new();

        let now = SystemTime::now();
        let mut removed = 0;

        unsafe {
            collect_dir(
                root_fd,
                &mut rel_components,
                &mut files,
                &mut total_size,
                now,
                tmp_grace_period,
                &mut removed,
            );
        }

        // 1. Remove files older than max_age
        files.retain(|f| {
            if let Ok(age) = now.duration_since(f.modified) {
                if age > max_age {
                    if remove_entry(root_fd, f) {
                        total_size = total_size.saturating_sub(f.size);
                        removed += 1;
                        return false;
                    }
                }
            }
            true
        });

        // 2. If total size still exceeds max_size_bytes, evict oldest first
        if total_size > max_size_bytes {
            files.sort_by_key(|f| f.modified);
            for f in &files {
                if total_size <= max_size_bytes {
                    break;
                }
                if remove_entry(root_fd, f) {
                    total_size = total_size.saturating_sub(f.size);
                    removed += 1;
                }
            }
        }

        unsafe {
            libc::close(root_fd);
        }

        Ok(removed)
    }

    unsafe fn collect_dir(
        current_fd: libc::c_int,
        rel_components: &mut Vec<std::ffi::CString>,
        files: &mut Vec<CacheEntry>,
        total_size: &mut u64,
        now: SystemTime,
        tmp_grace_period: Duration,
        removed: &mut usize,
    ) {
        unsafe {
            let dup_fd = libc::dup(current_fd);
            if dup_fd < 0 {
                return;
            }
            let dir_stream = libc::fdopendir(dup_fd);
            if dir_stream.is_null() {
                libc::close(dup_fd);
                return;
            }

            loop {
                let entry = libc::readdir(dir_stream);
                if entry.is_null() {
                    break;
                }
                let name_ptr = (*entry).d_name.as_ptr();
                let name = std::ffi::CStr::from_ptr(name_ptr);
                let bytes = name.to_bytes();
                if bytes == b"." || bytes == b".." {
                    continue;
                }
                if bytes.ends_with(b".lock") {
                    continue;
                }
                let is_tmp_stub = bytes.starts_with(b".tmp-stub-");
                if bytes.starts_with(b".") && !is_tmp_stub {
                    continue;
                }

                let mut st: libc::stat = std::mem::zeroed();
                if libc::fstatat(
                    current_fd,
                    name.as_ptr(),
                    &mut st,
                    libc::AT_SYMLINK_NOFOLLOW,
                ) != 0
                {
                    continue;
                }

                let mode = st.st_mode & libc::S_IFMT;
                if mode == libc::S_IFLNK {
                    continue;
                }

                if is_tmp_stub {
                    if mode == libc::S_IFREG {
                        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
                        let file_size = st.st_size as u64;
                        let name_str = name.to_str().unwrap_or("");
                        let created_at = parse_tmp_stub_timestamp(name_str);
                        let age = match created_at {
                            Some(ts) => now.duration_since(ts).unwrap_or_else(|_| {
                                now.duration_since(modified).unwrap_or(Duration::ZERO)
                            }),
                            None => now.duration_since(modified).unwrap_or(Duration::ZERO),
                        };

                        // Check whether an active publisher holds an OS lock/lease on this temporary file
                        let is_locked = is_temp_file_locked(current_fd, name.as_ptr());

                        if !is_locked && age > tmp_grace_period {
                            // Abandoned temporary file: unlink immediately
                            if libc::unlinkat(current_fd, name.as_ptr(), 0) == 0 {
                                *removed += 1;
                            }
                        } else {
                            // Active publisher holding lock or recent temporary file: account for disk space in cache budget,
                            // but DO NOT add to eviction candidate list `files` to protect active publishers.
                            *total_size += file_size;
                        }
                    }
                    continue;
                }

                if mode == libc::S_IFDIR {
                    let child_fd = libc::openat(
                        current_fd,
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    );
                    if child_fd >= 0 {
                        rel_components.push(name.to_owned());
                        collect_dir(
                            child_fd,
                            rel_components,
                            files,
                            total_size,
                            now,
                            tmp_grace_period,
                            removed,
                        );
                        rel_components.pop();
                        libc::close(child_fd);
                    }
                } else if mode == libc::S_IFREG {
                    let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(st.st_mtime.max(0) as u64);
                    let size = st.st_size as u64;
                    *total_size += size;
                    files.push(CacheEntry {
                        rel_components: rel_components.clone(),
                        file_name: name.to_owned(),
                        size,
                        modified,
                    });
                }
            }

            libc::closedir(dir_stream);
        }
    }

    fn remove_entry(root_fd: libc::c_int, entry: &CacheEntry) -> bool {
        let mut current_fd = root_fd;
        let mut fds_to_close = Vec::new();

        for comp in &entry.rel_components {
            let next_fd = unsafe {
                libc::openat(
                    current_fd,
                    comp.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if next_fd < 0 {
                for fd in fds_to_close {
                    unsafe { libc::close(fd); }
                }
                return false;
            }
            fds_to_close.push(next_fd);
            current_fd = next_fd;
        }

        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let is_reg = unsafe {
            libc::fstatat(
                current_fd,
                entry.file_name.as_ptr(),
                &mut st,
                libc::AT_SYMLINK_NOFOLLOW,
            ) == 0
                && (st.st_mode & libc::S_IFMT) == libc::S_IFREG
        };

        let removed = if is_reg {
            unsafe { libc::unlinkat(current_fd, entry.file_name.as_ptr(), 0) == 0 }
        } else {
            false
        };

        for fd in fds_to_close {
            unsafe { libc::close(fd); }
        }

        removed
    }

    unsafe fn is_temp_file_locked(dir_fd: libc::c_int, name: *const libc::c_char) -> bool {
        unsafe {
            let fd = libc::openat(
                dir_fd,
                name,
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            );
            if fd < 0 {
                // Cannot open descriptor (e.g. concurrency or permissions) - err on side of caution
                return true;
            }

            let ret = libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB);
            if ret == 0 {
                // Successfully acquired lock: no other process holds an exclusive lock
                libc::flock(fd, libc::LOCK_UN);
                libc::close(fd);
                false
            } else {
                // Lock attempt failed: an active publisher holds an exclusive lock on this file!
                libc::close(fd);
                true
            }
        }
    }
}

#[cfg(not(unix))]
fn prune_fallback(
    cache_dir: &Path,
    max_age: Duration,
    max_size_bytes: u64,
    tmp_grace_period: Duration,
) -> io::Result<usize> {
    if !cache_dir.is_dir() {
        return Ok(0);
    }
    let now = SystemTime::now();
    let mut removed = 0;
    let mut files = Vec::new();
    let mut total_size = 0u64;

    fn walk(
        dir: &Path,
        files: &mut Vec<(PathBuf, u64, SystemTime)>,
        total_size: &mut u64,
        now: SystemTime,
        tmp_grace_period: Duration,
        removed: &mut usize,
    ) {
        let Ok(entries) = fs::read_dir(dir) else { return; };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else { continue; };
            if file_type.is_symlink() { continue; }
            let path = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with(".lock") { continue; }
            let is_tmp_stub = name_str.starts_with(".tmp-stub-");
            if name_str.starts_with('.') && !is_tmp_stub { continue; }

            if file_type.is_dir() {
                walk(&path, files, total_size, now, tmp_grace_period, removed);
            } else if file_type.is_file() {
                let Ok(meta) = fs::metadata(&path) else { continue; };
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let size = meta.len();
                let created_at = parse_tmp_stub_timestamp(&name_str);
                let age = match created_at {
                    Some(ts) => now.duration_since(ts).unwrap_or_else(|_| {
                        now.duration_since(modified).unwrap_or(Duration::ZERO)
                    }),
                    None => now.duration_since(modified).unwrap_or(Duration::ZERO),
                };

                if is_tmp_stub {
                    // On non-Unix, check if file is exclusively locked by testing write access
                    let is_locked = fs::OpenOptions::new()
                        .write(true)
                        .open(&path)
                        .is_err();

                    if !is_locked && age > tmp_grace_period {
                        if fs::remove_file(&path).is_ok() {
                            *removed += 1;
                        }
                    } else {
                        *total_size += size;
                    }
                } else {
                    *total_size += size;
                    files.push((path, size, modified));
                }
            }
        }
    }

    walk(cache_dir, &mut files, &mut total_size, now, tmp_grace_period, &mut removed);

    // Evict files older than max_age
    files.retain(|(path, size, modified)| {
        if let Ok(age) = now.duration_since(*modified) {
            if age > max_age {
                if fs::remove_file(path).is_ok() {
                    *total_size = total_size.saturating_sub(*size);
                    removed += 1;
                    return false;
                }
            }
        }
        true
    });

    // Evict oldest stubs if still over budget
    if total_size > max_size_bytes {
        files.sort_by_key(|(_, _, modified)| *modified);
        for (path, size, _) in files {
            if total_size <= max_size_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total_size = total_size.saturating_sub(size);
                removed += 1;
            }
        }
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_python_stub_cache_env_and_path() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let custom_cache = temp.path().join("my-custom-python-stubs");

        unsafe { std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache); }
        let dir = python_stub_cache_dir();
        assert_eq!(dir, custom_cache);
        assert!(dir.is_dir());

        let envs = python_stub_cache_env();
        let custom_str = custom_cache.to_str().unwrap();
        assert!(envs.iter().any(|(k, v)| k == PYTHON_STUB_CACHE_ENV && v == custom_str));
        assert!(envs.iter().any(|(k, v)| k == "MYPYPATH" && v == custom_str));
        assert!(envs.iter().any(|(k, v)| k == "TYPINGS_PATH" && v == custom_str));

        unsafe { std::env::remove_var(PYTHON_STUB_CACHE_ENV); }
    }

    #[test]
    fn test_is_python_project_detection() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        assert!(!is_python_project(root));

        fs::write(root.join("pyproject.toml"), "[project]\nname=\"foo\"\n").unwrap();
        assert!(is_python_project(root));
    }

    #[test]
    fn test_merge_stubs_and_seed_worktree() {
        let _guard = TEST_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let custom_cache = temp.path().join("shared-stubs");
        unsafe { std::env::set_var(PYTHON_STUB_CACHE_ENV, &custom_cache); }

        let from = temp.path().join("origin-project");
        let to = temp.path().join("worktree-project");
        fs::create_dir_all(&from).unwrap();
        fs::write(from.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();

        // Add local typings
        let from_typings = from.join("typings").join("requests");
        fs::create_dir_all(&from_typings).unwrap();
        fs::write(from_typings.join("__init__.pyi"), "def get(url: str): ...\n").unwrap();

        // Run seed
        let result = seed_python_worktree(&from, &to).unwrap();
        assert!(result.is_some());

        // Verify stubs were merged into shared cache
        assert!(custom_cache.join("requests").join("__init__.pyi").is_file());

        // Verify to/typings symlink points to shared cache
        let to_typings = to.join("typings");
        assert!(fs::symlink_metadata(&to_typings).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&to_typings).unwrap(), custom_cache);

        unsafe { std::env::remove_var(PYTHON_STUB_CACHE_ENV); }
    }

    #[test]
    fn test_parse_tmp_stub_timestamp() {
        let now = SystemTime::now();
        let nanos = now.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_nanos();
        let name = format!(".tmp-stub-1234-5-{nanos:x}");
        let parsed = parse_tmp_stub_timestamp(&name).unwrap();
        let diff = if now > parsed {
            now.duration_since(parsed).unwrap()
        } else {
            parsed.duration_since(now).unwrap()
        };
        assert!(diff < Duration::from_millis(1));

        assert!(parse_tmp_stub_timestamp("regular.pyi").is_none());
        assert!(parse_tmp_stub_timestamp(".tmp-stub-invalid").is_none());
        assert!(parse_tmp_stub_timestamp(".tmp-stub-active-worker-2").is_none());
    }
}
