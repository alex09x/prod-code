/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

/// Node paths whose content never changes once there: a copy of one is never fetched again.
const IMMUTABLE: &[&str] = &[
    "/.cargo/registry/",
    "/.cargo/git/",
    "/.rustup/toolchains/",
    "/go/pkg/mod/",
];

/// The node's files for one editor session, mirrored under a local directory.
pub struct RemoteFiles {
    remote: std::sync::RwLock<SocketAddr>,
    /// The node's absolute paths live under this directory.
    mirror: std::sync::RwLock<PathBuf>,
    mirror_paths: std::sync::RwLock<prod_code_protocol::path::PathTranslator>,
    cache: PathBuf,
    client_root: PathBuf,
    server_root: std::sync::RwLock<PathBuf>,
}

/// Where the node's files are mirrored by default: the user's cache directory.
pub fn default_cache() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/prod-code/remote")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".cache"))
            .join("prod-code/remote")
    }
}

impl RemoteFiles {
    /// The files of the node at `remote` for a checkout at `client_root`, whose copy on the node
    /// is `server_root`, mirrored under `cache` (one directory per node).
    pub fn new(remote: SocketAddr, client_root: &Path, server_root: &Path, cache: &Path) -> Self {
        let mirror = cache.join(remote.to_string().replace(':', "_"));
        Self {
            remote: std::sync::RwLock::new(remote),
            mirror_paths: std::sync::RwLock::new(prod_code_protocol::path::PathTranslator::new(
                &mirror.to_string_lossy(),
                "/",
            )),
            mirror: std::sync::RwLock::new(mirror),
            cache: cache.to_path_buf(),
            client_root: client_root.to_path_buf(),
            server_root: std::sync::RwLock::new(server_root.to_path_buf()),
        }
    }

    /// Changes the remote node after a gateway redirect, including the server root used to
    /// translate workspace paths and the per-node external-file cache directory.
    pub fn set_node(&self, remote: SocketAddr, server_root: &Path) {
        let mirror = self.cache.join(remote.to_string().replace(':', "_"));
        *self.remote.write().unwrap_or_else(|p| p.into_inner()) = remote;
        *self.server_root.write().unwrap_or_else(|p| p.into_inner()) = server_root.to_path_buf();
        *self.mirror_paths.write().unwrap_or_else(|p| p.into_inner()) =
            prod_code_protocol::path::PathTranslator::new(&mirror.to_string_lossy(), "/");
        *self.mirror.write().unwrap_or_else(|p| p.into_inner()) = mirror;
    }

    /// The editor's message with every path of a mirrored copy turned back into the node's.
    pub fn to_node(&self, raw: &str) -> String {
        self.mirror_paths
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .translate_lsp_to_server(raw)
    }

    /// The node path a `file://` URI of a server's message names, when the editor cannot open
    /// it: a path outside the checkout, or one inside it that only the node's copy has.
    pub fn node_path(&self, uri: &str) -> Option<PathBuf> {
        if uri.contains("/..") || uri.contains("../") || uri.contains("%2e") || uri.contains("%2E")
        {
            return None;
        }
        let path = PathBuf::from(prod_code_mcp::remote_fs::uri_to_path(uri));
        if path.starts_with(&*self.mirror.read().unwrap_or_else(|p| p.into_inner())) {
            return None;
        }
        let server_root = self.server_root.read().unwrap_or_else(|p| p.into_inner());
        match path.strip_prefix(&self.client_root) {
            Ok(_) if path.exists() => None,
            Ok(rel) => {
                let mut clean = PathBuf::new();
                for comp in rel.components() {
                    match comp {
                        std::path::Component::Normal(c) => clean.push(c),
                        std::path::Component::CurDir => {}
                        std::path::Component::ParentDir => {
                            if !clean.pop() {
                                return None;
                            }
                        }
                        _ => {}
                    }
                }
                Some(server_root.join(clean))
            }
            Err(_) => {
                if path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return None;
                }
                Some(path)
            }
        }
    }

    /// Where the copy of the node's `node` path lives, safely contained within the mirror root.
    pub fn mirror_path(&self, node: &Path) -> Option<PathBuf> {
        let mirror = self
            .mirror
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let mut clean = PathBuf::new();
        for comp in node.components() {
            match comp {
                std::path::Component::Normal(c) => clean.push(c),
                std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if !clean.pop() {
                        return None;
                    }
                }
            }
        }
        let target = mirror.join(&clean);
        if target.starts_with(&mirror) {
            Some(target)
        } else {
            None
        }
    }

    /// The local copy of the file a URI names, fetched from the node unless an unchanging copy
    /// is there already; `None` when the editor can open the URI itself or the node has no
    /// such file to give.
    async fn copy(&self, uri: &str) -> Option<String> {
        let node = self.node_path(uri)?;
        let local = self.mirror_path(&node)?;
        let mirror_root = self
            .mirror
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        if local == mirror_root {
            return None;
        }
        let node_text = node.to_string_lossy();
        let immutable = IMMUTABLE.iter().any(|part| node_text.contains(part));
        if !(immutable && local.is_file()) {
            let remote = *self.remote.read().unwrap_or_else(|p| p.into_inner());
            let (bytes, _truncated) =
                prod_code_mcp::remote_fs::read_remote_file(remote, &node_text, 0)
                    .await
                    .ok()?;
            write_read_only(&mirror_root, &local, &bytes).ok()?;
        }
        url::Url::from_file_path(&local).ok().map(|u| u.to_string())
    }

    /// The server's message with each node path the editor cannot open replaced by the path
    /// of its local copy.
    pub async fn to_editor(&self, raw: String) -> String {
        if !raw.contains("file://") {
            return raw;
        }
        let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&raw) else {
            return raw;
        };
        let mut uris = Vec::new();
        prod_code_protocol::path::map_lsp_locations(&mut value, &mut |text| {
            if text.starts_with("file://") {
                uris.push(text.to_string());
            }
            None
        });
        let mut copies = HashMap::new();
        for uri in uris {
            if copies.contains_key(&uri) {
                continue;
            }
            if let Some(local) = self.copy(&uri).await {
                copies.insert(uri, local);
            }
        }
        if copies.is_empty() {
            return raw;
        }
        prod_code_protocol::path::map_lsp_locations(&mut value, &mut |text| {
            copies.get(text).cloned()
        });
        value.to_string()
    }
}

/// Writes a copy that is read-only, safely verifying it does not traverse outside the mirror root.
pub fn write_read_only(mirror_root: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if !path.starts_with(mirror_root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "mirror path escapes mirror root",
        ));
    }
    if let Ok(meta) = path.symlink_metadata() {
        if meta.file_type().is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "refusing to write mirror file through symlink",
            ));
        }
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        if let Ok(canon_parent) = std::fs::canonicalize(parent) {
            if let Ok(canon_root) = std::fs::canonicalize(mirror_root) {
                if !canon_parent.starts_with(&canon_root) {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "canonical mirror path escapes mirror root",
                    ));
                }
            }
        }
    }
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o444))?;
    Ok(())
}
