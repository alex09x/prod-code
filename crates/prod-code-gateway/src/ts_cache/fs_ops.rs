/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::fs;
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use super::env::ensure_cache_dir;

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
fn sync_mtime_from_meta(meta: &fs::Metadata, dst: &Path) {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;
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

#[cfg(not(unix))]
fn sync_mtime_from_meta(_meta: &fs::Metadata, _dst: &Path) {}

static TYPE_FILE_NONCE: AtomicU64 = AtomicU64::new(1);

/// Copies a type declaration file to `dst` atomically under a per-target lock,
/// ensuring concurrent seeders do not race and that older files never overwrite newer files.
/// Operates on the open file handle directly to prevent symlink TOCTOU races (#836).
pub(crate) fn copy_and_publish_type_file(src: &Path, dst: &Path) -> io::Result<u64> {
    let parent = dst.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination file has no parent directory",
        )
    })?;
    ensure_cache_dir(parent)?;

    let file_name = dst.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "destination file has no name")
    })?;
    let lock_path = parent.join(format!(".lock-{}", file_name.to_string_lossy()));
    let _lock = TargetLock::acquire(&lock_path)?;

    // Open source file first; all subsequent metadata and copy operations use the open file handle (#836)
    let mut src_file = fs::File::open(src)?;
    let src_meta = src_file.metadata()?;
    let src_mod = src_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

    if let Ok(dst_meta) = fs::metadata(dst) {
        let dst_mod = dst_meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if dst_mod > src_mod || (dst_mod == src_mod && dst_meta.len() == src_meta.len()) {
            return Ok(0);
        }
    }

    let nonce = TYPE_FILE_NONCE.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp_name = format!(".tmp-ts-{pid}-{nonce}-{ts:x}");
    let tmp_path = parent.join(tmp_name);

    let mut tmp_file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;

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

    sync_mtime_from_meta(&src_meta, dst);

    Ok(bytes)
}
