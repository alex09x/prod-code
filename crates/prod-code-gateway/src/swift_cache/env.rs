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
use std::path::{Path, PathBuf};

/// The environment variable name used to explicitly configure the shared Swift module cache directory.
pub const SWIFT_MODULE_CACHE_ENV: &str = "PROD_CODE_SWIFT_MODULE_CACHE";

/// Returns the path to the shared Swift and Clang module cache directory.
///
/// Precedence:
/// 1. `PROD_CODE_SWIFT_MODULE_CACHE` environment variable if set.
/// 2. `SWIFTPM_MODULECACHE_OVERRIDE` environment variable if already configured.
/// 3. `$HOME/.cache/prod-code/swift-module-cache`.
/// 4. A UID-specific cache directory under `/var/tmp`.
/// 5. A process-specific fallback under the system temporary directory.
///
/// Ensures the directory exists with mode `0700` on Unix systems.
fn resolve_cache_dir(path: PathBuf) -> Option<PathBuf> {
    if path.as_os_str().is_empty() {
        return None;
    }
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    if path.file_name().is_none() || std::env::current_dir().ok().as_deref() == Some(path.as_path())
    {
        return None;
    }
    match ensure_cache_dir(&path) {
        Ok(()) => Some(path),
        Err(error) => {
            tracing::warn!(cache = %path.display(), %error, "rejecting insecure Swift module cache directory");
            None
        }
    }
}

pub fn swift_module_cache_dir() -> PathBuf {
    if let Some(custom) = std::env::var_os(SWIFT_MODULE_CACHE_ENV) {
        if let Some(dir) = resolve_cache_dir(PathBuf::from(custom)) {
            return dir;
        }
    }

    if let Some(override_path) = std::env::var_os("SWIFTPM_MODULECACHE_OVERRIDE") {
        if let Some(dir) = resolve_cache_dir(PathBuf::from(override_path)) {
            return dir;
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("swift-module-cache");
        if let Some(dir) = resolve_cache_dir(p) {
            return dir;
        }
    }

    let user = cache_user_suffix();
    let var_tmp = PathBuf::from("/var/tmp").join(format!("prod-code-swift-module-cache-{user}"));
    if let Some(dir) = resolve_cache_dir(var_tmp) {
        return dir;
    }

    let temp = std::env::temp_dir().join(format!(
        "prod-code-swift-module-cache-{user}-{}",
        std::process::id()
    ));
    resolve_cache_dir(temp).expect("no private Swift module cache directory could be created")
}

fn cache_user_suffix() -> String {
    #[cfg(unix)]
    {
        // SAFETY: geteuid has no preconditions.
        unsafe { libc::geteuid() }.to_string()
    }
    #[cfg(not(unix))]
    {
        std::env::var("USERNAME")
            .or_else(|_| std::env::var("USER"))
            .unwrap_or_else(|_| "default".to_string())
    }
}

/// Ensures `dir` exists and has secure permissions (`0700` on Unix).
pub fn ensure_cache_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() || dir.file_name().is_none() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid cache directory path",
        ));
    }
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::MetadataExt;
        let c_path = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: fd was returned by open and ownership is transferred to File.
        let directory = unsafe { std::fs::File::from_raw_fd(fd) };
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Swift module cache directory is not owned by the current user",
            ));
        }
        if metadata.mode() & 0o077 != 0
            && unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let secured = directory.metadata()?;
        if !secured.is_dir()
            || secured.uid() != unsafe { libc::geteuid() }
            || secured.mode() & 0o077 != 0
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Swift module cache directory could not be secured to mode 0700",
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Swift module cache path is not a real directory",
            ));
        }
    }
    Ok(())
}

/// Returns the environment variables that configure the shared Swift and Clang module cache
/// across SwiftPM (`swift build`, `swift test`, `swift run`), `swiftc`, ClangImporter,
/// and `sourcekit-lsp` (Roadmap 3.7).
pub fn swift_module_cache_env() -> Vec<(String, String)> {
    let dir = swift_module_cache_dir();
    let dir_str = dir.to_string_lossy().into_owned();
    vec![
        // SwiftPM module cache override (passes -module-cache-path to swiftc and -fmodules-cache-path to clang)
        ("SWIFTPM_MODULECACHE_OVERRIDE".to_string(), dir_str.clone()),
        // Direct Swift compiler module cache path
        ("SWIFT_MODULE_CACHE_PATH".to_string(), dir_str.clone()),
        // Clang importer / C module cache path
        ("CLANG_MODULE_CACHE_PATH".to_string(), dir_str),
    ]
}
