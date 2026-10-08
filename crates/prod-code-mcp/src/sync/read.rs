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
use std::path::Path;

#[cfg(unix)]
pub(crate) fn verify_fd_containment(file: &std::fs::File, canonical_root: &Path) -> Result<bool> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();

    #[cfg(target_os = "macos")]
    {
        let mut buf = vec![0u8; libc::PATH_MAX as usize];
        if unsafe { libc::fcntl(fd, libc::F_GETPATH, buf.as_mut_ptr() as *mut libc::c_char) } != -1
        {
            let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            if let Ok(path_str) = std::str::from_utf8(&buf[..len]) {
                return Ok(Path::new(path_str).starts_with(canonical_root));
            }
        }
        Ok(false)
    }

    #[cfg(target_os = "linux")]
    {
        if let Ok(link) = std::fs::read_link(format!("/proc/self/fd/{fd}")) {
            return Ok(link.starts_with(canonical_root));
        }
        Ok(false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = fd;
        let _ = canonical_root;
        anyhow::bail!("File descriptor containment verification is not supported on this platform");
    }
}

/// Securely opens and reads a regular file without following symlinks to eliminate symlink TOCTOU.
/// Rejects symlinks at opening via `O_NOFOLLOW` and reads directly from the verified file handle.
pub(crate) fn read_regular_file_secure(
    path: &Path,
    canonical_root: &Path,
    relative_path: &str,
) -> Result<Option<(Vec<u8>, bool)>> {
    #[cfg(not(unix))]
    {
        let _ = path;
        let _ = canonical_root;
        let _ = relative_path;
        anyhow::bail!("Secure file reading is unsupported on non-Unix platforms");
    }

    #[cfg(unix)]
    {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);

        let mut file = match options.open(path) {
            Ok(f) => f,
            Err(err) => {
                if err.raw_os_error() == Some(libc::ELOOP) {
                    return Ok(None);
                }
                return Ok(None);
            }
        };

        let metadata = match file.metadata() {
            Ok(m) => m,
            Err(_) => return Ok(None),
        };

        if !metadata.file_type().is_file() {
            return Ok(None);
        }

        if !crate::sync::entry::fits_sync(relative_path, &metadata) {
            return Ok(None);
        }

        if !verify_fd_containment(&file, canonical_root)? {
            return Ok(None);
        }

        let limit = crate::sync::entry::size_limit(relative_path);
        let mut content = Vec::with_capacity(std::cmp::min(metadata.len(), limit) as usize);
        use std::io::Read;
        if (&mut file)
            .take(limit + 1)
            .read_to_end(&mut content)
            .is_err()
        {
            return Ok(None);
        }
        if content.len() as u64 > limit {
            return Ok(None);
        }

        let is_exec = crate::sync::entry::is_executable(&metadata);
        Ok(Some((content, is_exec)))
    }
}

/// Securely reads a regular file or a Git-tracked symlink whose resolved target
/// remains contained within `canonical_root`.
pub(crate) fn read_file_or_contained_symlink(
    path: &Path,
    canonical_root: &Path,
    relative_path: &str,
) -> Result<Option<(Vec<u8>, bool, std::fs::Metadata)>> {
    let sym_meta = match path.symlink_metadata() {
        Ok(m) => m,
        Err(_) => return Ok(None),
    };
    if sym_meta.file_type().is_symlink() {
        if let Ok(canonical_target) = std::fs::canonicalize(path) {
            if canonical_target.starts_with(canonical_root) && canonical_target.is_file() {
                if let Ok(target_meta) = canonical_target.metadata() {
                    if !crate::sync::entry::fits_sync(relative_path, &target_meta) {
                        return Ok(None);
                    }
                    if let Some((content, is_exec)) =
                        read_regular_file_secure(&canonical_target, canonical_root, relative_path)?
                    {
                        return Ok(Some((content, is_exec, target_meta)));
                    }
                }
            }
        }
        Ok(None)
    } else if sym_meta.file_type().is_file() {
        if !crate::sync::entry::fits_sync(relative_path, &sym_meta) {
            return Ok(None);
        }
        if let Some((content, is_exec)) =
            read_regular_file_secure(path, canonical_root, relative_path)?
        {
            return Ok(Some((content, is_exec, sym_meta)));
        }
        Ok(None)
    } else {
        Ok(None)
    }
}
