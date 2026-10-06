/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::*;

/// Whether `path` is a source file the gateway may hand to clients: its own workspace
/// copies, toolchain and dependency caches under the home directory, and system SDK
/// locations. Nothing else on the host is readable this way.
pub fn is_readable_source_path(storage_root: &std::path::Path, path: &std::path::Path) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    is_readable_source_path_with_home(storage_root, path, home.as_deref())
}

pub fn is_readable_source_path_with_home(
    storage_root: &std::path::Path,
    path: &std::path::Path,
    home: Option<&std::path::Path>,
) -> bool {
    if !path.is_absolute() || !path.is_file() {
        return false;
    }
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    if canonical.starts_with(storage_root) {
        return true;
    }
    if let Some(home) = home {
        let allowed_home = [
            ".cargo/registry",
            ".cargo/git",
            ".rustup/toolchains",
            "go/pkg/mod",
            ".local/go",
            ".local/lib",
            ".npm-global/lib",
            ".bun/install",
            ".pnpm-store",
            ".local/share/pnpm",
            ".yarn/cache",
            ".cache/yarn",
            ".local/share/uv",
            ".cache/uv",
            ".cache/pypoetry",
            ".virtualenvs",
            ".pyenv",
            ".local/share/virtualenvs",
            ".local/pipx",
            ".conda",
            ".nvm",
            ".fnm",
            "Library/Developer",
            "prod-code-storage",
        ];
        if allowed_home
            .iter()
            .any(|rel| canonical.starts_with(home.join(rel)))
        {
            return true;
        }
    }
    if is_gopath_source_path(&canonical, std::env::var_os("GOPATH").as_deref()) {
        return true;
    }
    if let Some(goroot) = std::env::var_os("GOROOT") {
        let goroot = PathBuf::from(goroot);
        if canonical.starts_with(&goroot) {
            return true;
        }
    }
    const SYSTEM_ROOTS: [&str; 14] = [
        "/snap",
        "/usr/include",
        "/usr/local/include",
        "/usr/local/Cellar",
        "/usr/lib",
        "/usr/local/lib",
        "/usr/local/go",
        "/usr/share",
        "/opt/homebrew",
        "/Applications/Xcode.app",
        "/Library/Developer",
        "/Library/Frameworks",
        "/System/Library/Frameworks",
        "/opt/conda",
    ];
    if SYSTEM_ROOTS.iter().any(|root| canonical.starts_with(root)) {
        return true;
    }
    false
}

pub fn is_gopath_source_path(path: &std::path::Path, gopath: Option<&std::ffi::OsStr>) -> bool {
    gopath.is_some_and(|value| {
        std::env::split_paths(value).any(|root| {
            path.starts_with(root.join("pkg/mod")) || path.starts_with(root.join("src"))
        })
    })
}

/// Serves a `ReadFileRequest` under the readable-path policy, capped in size.
pub fn read_server_file(
    storage_root: &std::path::Path,
    req: &prod_code_protocol::ReadFileRequest,
) -> prod_code_protocol::ReadFileResponse {
    pub const DEFAULT_MAX_SOURCE: u64 = 2 * 1024 * 1024;
    pub const MAX_PULL_BYTES: u64 = 64 * 1024 * 1024;
    let path = PathBuf::from(&req.path);
    let mut resp = prod_code_protocol::ReadFileResponse {
        path: req.path.clone(),
        content: None,
        truncated: false,
        is_executable: None,
        error: None,
    };
    if !is_readable_source_path(storage_root, &path) {
        resp.error = Some(format!(
            "{} is not a readable source location on this gateway",
            req.path
        ));
        return resp;
    }
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let in_workspace = canonical.starts_with(storage_root);
    let ceiling = if in_workspace {
        MAX_PULL_BYTES
    } else {
        DEFAULT_MAX_SOURCE
    };
    let max = if req.max_bytes == 0 {
        ceiling
    } else {
        req.max_bytes.min(ceiling)
    };
    match std::fs::read(&path) {
        Ok(mut bytes) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Ok(metadata) = std::fs::metadata(&path) {
                    resp.is_executable = Some(metadata.permissions().mode() & 0o111 != 0);
                }
            }
            if bytes.len() as u64 > max {
                bytes.truncate(max as usize);
                resp.truncated = true;
            }
            resp.content = Some(bytes);
        }
        Err(e) => resp.error = Some(format!("cannot read {}: {e}", req.path)),
    }
    resp
}
