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
        if let Some(path) = resolve_python_cache_dir(PathBuf::from(custom)) {
            return path;
        }
    }

    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home)
            .join(".cache")
            .join("prod-code")
            .join("python-stubs");
        if let Some(path) = resolve_python_cache_dir(p) {
            return path;
        }
    }

    let user = cache_user_suffix();
    let var_tmp = PathBuf::from("/var/tmp").join(format!("prod-code-python-stubs-{user}"));
    if let Some(path) = resolve_python_cache_dir(var_tmp) {
        return path;
    }

    let temp = std::env::temp_dir().join(format!(
        "prod-code-python-stubs-{user}-{}",
        std::process::id()
    ));
    resolve_python_cache_dir(temp).expect("no private Python stub cache directory could be created")
}

fn resolve_python_cache_dir(path: PathBuf) -> Option<PathBuf> {
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
    ensure_cache_dir(&path).ok()?;
    Some(path)
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
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    }
}

pub(crate) fn is_shared_stub_cache_link(path: &Path, cache_root: &Path) -> bool {
    if !fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return false;
    }
    let (Ok(target), Ok(cache_root)) = (fs::canonicalize(path), fs::canonicalize(cache_root))
    else {
        return false;
    };
    target.starts_with(cache_root)
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
        let path = std::ffi::CString::new(dir.as_os_str().as_bytes())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
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
                "Python stub cache directory is not owned by the current user",
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
                "Python stub cache directory could not be secured to mode 0700",
            ));
        }
    }
    #[cfg(not(unix))]
    {
        let metadata = fs::symlink_metadata(dir)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Python stub cache path is not a real directory",
            ));
        }
    }
    Ok(())
}

/// Returns the environment variables that configure the shared Python stub cache
/// across basedpyright, pyright, mypy, and Python type checkers (Roadmap 3.6).
pub fn python_stub_cache_env() -> Vec<(String, String)> {
    python_stub_cache_env_for_dir(&python_stub_cache_dir())
}

/// Uses the workspace's version-isolated typings view when starting its type checker.
pub fn python_stub_cache_env_for_workspace(workspace: &Path) -> Vec<(String, String)> {
    python_stub_cache_env_for_dir(&workspace.join("typings"))
}

pub fn python_stub_cache_env_for_dir(dir: &Path) -> Vec<(String, String)> {
    let dir_str = dir.to_string_lossy().into_owned();
    let mypypath = mypypath_view_for_dir(dir).to_string_lossy().into_owned();
    vec![
        (PYTHON_STUB_CACHE_ENV.to_string(), dir_str.clone()),
        ("MYPYPATH".to_string(), mypypath),
        ("TYPINGS_PATH".to_string(), dir_str),
    ]
}

fn mypypath_view_for_dir(dir: &Path) -> PathBuf {
    let mypy_dir = dir.join(".mypypath");
    if is_mypypath_view_ready(&mypy_dir) {
        return mypy_dir;
    }

    if has_stub_packages(dir) {
        if let Ok(view) = ensure_mypypath_view(dir) {
            if is_mypypath_view_ready(&view) {
                return view;
            }
        }
    }

    dir.to_path_buf()
}

fn is_mypypath_view_ready(mypy_dir: &Path) -> bool {
    mypy_dir.join(".ready").is_file()
}

fn has_stub_packages(dir: &Path) -> bool {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.ends_with("-stubs") {
                return true;
            }
        }
    }
    false
}

/// Creates or updates an MYPYPATH-compatible view within `cache_dir`, exposing
/// PEP 561 stub packages (`*-stubs`) under their canonical Python import names
/// without modifying the PEP 561 layout used by Pyright.
pub fn ensure_mypypath_view(cache_dir: &Path) -> io::Result<PathBuf> {
    let mypy_dir = cache_dir.join(".mypypath");
    ensure_cache_dir(&mypy_dir)?;

    let entries = fs::read_dir(cache_dir)?;

    let mut import_map: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    let mut direct_files: Vec<String> = Vec::new();

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') {
            continue;
        }

        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() || file_type.is_symlink() {
            if let Some(import_name) = name_str.strip_suffix("-stubs") {
                import_map
                    .entry(import_name.to_string())
                    .or_insert_with(|| name_str.to_string());
            } else {
                import_map.insert(name_str.to_string(), name_str.to_string());
            }
        } else if file_type.is_file() && name_str.ends_with(".pyi") {
            direct_files.push(name_str.to_string());
        }
    }

    let ready_marker = mypy_dir.join(".ready");
    let _ = fs::remove_file(&ready_marker);

    #[cfg(unix)]
    {
        for (import_name, target_entry) in &import_map {
            let link_path = mypy_dir.join(import_name);
            let target_rel = Path::new("..").join(target_entry);

            let need_create = match fs::read_link(&link_path) {
                Ok(existing) => existing != target_rel,
                Err(_) => true,
            };

            if need_create {
                let _ = fs::remove_file(&link_path);
                let _ = fs::remove_dir_all(&link_path);
                std::os::unix::fs::symlink(&target_rel, &link_path)?;
            }
        }

        for file_name in &direct_files {
            let link_path = mypy_dir.join(file_name);
            let target_rel = Path::new("..").join(file_name);

            let need_create = match fs::read_link(&link_path) {
                Ok(existing) => existing != target_rel,
                Err(_) => true,
            };

            if need_create {
                let _ = fs::remove_file(&link_path);
                std::os::unix::fs::symlink(&target_rel, &link_path)?;
            }
        }
    }

    #[cfg(not(unix))]
    {
        populate_mypypath_view_copy(cache_dir, &mypy_dir, &import_map, &direct_files)?;
    }

    fs::write(&ready_marker, b"ready")?;
    Ok(mypy_dir)
}

#[allow(dead_code)]
pub(crate) fn populate_mypypath_view_copy(
    cache_dir: &Path,
    mypy_dir: &Path,
    import_map: &std::collections::BTreeMap<String, String>,
    direct_files: &[String],
) -> io::Result<()> {
    for (import_name, target_entry) in import_map {
        let link_path = mypy_dir.join(import_name);
        let target = cache_dir.join(target_entry);
        copy_dir_recursive(&target, &link_path)?;
    }
    for file_name in direct_files {
        let link_path = mypy_dir.join(file_name);
        let target = cache_dir.join(file_name);
        fs::copy(&target, &link_path)?;
    }
    Ok(())
}

#[allow(dead_code)]
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    if !dst.exists() {
        fs::create_dir_all(dst)?;
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}
