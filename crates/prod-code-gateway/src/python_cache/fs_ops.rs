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

static STUB_FILE_NONCE: AtomicU64 = AtomicU64::new(1);

/// Copies a file to `dst` atomically under a per-target lock, ensuring that concurrent seeders
/// do not race and that older files never overwrite newer files.
pub(crate) fn copy_and_publish_stub(src: &Path, dst: &Path) -> io::Result<u64> {
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
        let ret = unsafe { libc::flock(tmp_file.as_raw_fd(), libc::LOCK_EX) };
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

    Ok(bytes)
}
