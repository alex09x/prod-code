/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Reading source files that live only on the gateway host: what a definition outside the
//! checkout (standard library, dependency caches, SDK headers) points at.

use anyhow::{Context, Result, anyhow};
use futures_util::{SinkExt, StreamExt};
use prod_code_protocol::{ProdCodeCodec, ReadFileRequest, WireMessage};
use std::io::Read;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use tokio_util::codec::Framed;

/// Maximum bytes read from a local source file before truncating (matches gateway ReadFile limit).
pub const MAX_SOURCE_BYTES: u64 = 2 * 1024 * 1024;

/// Reads `path` on the gateway `remote`. Returns the bytes and whether they were truncated.
pub async fn read_remote_file(
    remote: SocketAddr,
    path: &str,
    max_bytes: u64,
) -> Result<(Vec<u8>, bool)> {
    let stream = prod_code_protocol::transport::connect(remote)
        .await
        .with_context(|| format!("failed to connect to remote gateway at {remote}"))?;
    let mut framed = Framed::new(stream, ProdCodeCodec::new());
    framed
        .send(WireMessage::ReadFileRequest(ReadFileRequest {
            path: path.to_string(),
            max_bytes,
        }))
        .await?;
    let reply = tokio::time::timeout(std::time::Duration::from_secs(15), framed.next())
        .await
        .map_err(|_| anyhow!("timed out reading {path} from {remote}"))?;
    match reply {
        Some(Ok(WireMessage::ReadFileResponse(resp))) => match (resp.content, resp.error) {
            (Some(bytes), _) => Ok((bytes, resp.truncated)),
            (None, Some(err)) => Err(anyhow!(err)),
            (None, None) => Err(anyhow!("empty reply for {path}")),
        },
        Some(Ok(other)) => Err(anyhow!("unexpected reply: {other:?}")),
        Some(Err(e)) => Err(anyhow!("decode error: {e}")),
        None => Err(anyhow!("gateway closed the connection")),
    }
}

/// Reads a source file. If `path_str` is relative or a local absolute path inside `root`,
/// it is read directly from disk in the local checkout (capped at 2 MiB).
/// If `path_str` is an external absolute path (e.g. stdlib, dependency cache, SDK headers),
/// it is fetched from the remote gateway via `read_remote_file`.
/// Relative paths that attempt to escape `root` are refused.
pub async fn read_source(
    remote: SocketAddr,
    root: &Path,
    path_str: &str,
) -> Result<(Vec<u8>, bool)> {
    let path = uri_to_path(path_str);
    let p = Path::new(&path);
    let root_canon = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());

    if p.is_absolute() {
        if let Some(local_path) = absolute_checkout_source_path(root, &root_canon, p)? {
            read_local_source_file(&local_path)
        } else {
            read_remote_file(remote, &path, 0).await
        }
    } else {
        let local_path = resolve_relative_checkout_path(root, &root_canon, p)?;
        read_local_source_file(&local_path)
    }
}

fn absolute_checkout_source_path(
    root: &Path,
    root_canon: &Path,
    path: &Path,
) -> Result<Option<PathBuf>> {
    let claims_checkout = path.starts_with(root) || path.starts_with(root_canon);
    if claims_checkout
        && path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        anyhow::bail!("{} is outside the workspace", path.display());
    }

    match std::fs::canonicalize(path) {
        Ok(canonical) if canonical.starts_with(root_canon) => Ok(Some(canonical)),
        Ok(_) if claims_checkout => {
            anyhow::bail!("{} is outside the workspace", path.display())
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && claims_checkout => {
            let ancestor = canonical_existing_ancestor(path)?;
            if !ancestor.starts_with(root_canon) {
                anyhow::bail!("{} is outside the workspace", path.display());
            }
            Err(error).with_context(|| format!("reading {}", path.display()))
        }
        Err(error) if claims_checkout => {
            Err(error).with_context(|| format!("resolving {}", path.display()))
        }
        Err(_) => Ok(None),
    }
}

fn canonical_existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut ancestor = path;
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                return std::fs::canonicalize(ancestor)
                    .with_context(|| format!("resolving {}", ancestor.display()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .context("source path has no existing ancestor")?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("resolving {}", ancestor.display()));
            }
        }
    }
}

fn read_local_source_file(path: &Path) -> Result<(Vec<u8>, bool)> {
    let file = std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    read_limited_source(file).with_context(|| format!("reading {}", path.display()))
}

fn read_limited_source(reader: impl Read) -> Result<(Vec<u8>, bool)> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("reading source content")?;
    let truncated = bytes.len() as u64 > MAX_SOURCE_BYTES;
    bytes.truncate(MAX_SOURCE_BYTES as usize);
    Ok((bytes, truncated))
}

fn resolve_relative_checkout_path(root: &Path, root_canon: &Path, p: &Path) -> Result<PathBuf> {
    let mut norm = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !norm.pop() {
                    anyhow::bail!("{} is outside the workspace", p.display());
                }
            }
            std::path::Component::Normal(c) => norm.push(c),
            _ => anyhow::bail!("{} is outside the workspace", p.display()),
        }
    }

    let candidate = if let Ok(cwd) = std::env::current_dir() {
        let cwd_canon = std::fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        if cwd_canon.starts_with(root_canon) && cwd.join(&norm).exists() {
            cwd.join(&norm)
        } else {
            root.join(&norm)
        }
    } else {
        root.join(&norm)
    };

    let resolved = if candidate.exists() {
        std::fs::canonicalize(&candidate)
            .with_context(|| format!("reading {}", candidate.display()))?
    } else {
        let mut cur = candidate.as_path();
        while !cur.exists() {
            if let Some(parent) = cur.parent() {
                cur = parent;
            } else {
                break;
            }
        }
        if let Ok(cur_canon) = std::fs::canonicalize(cur) {
            let outside = !cur_canon.starts_with(root) && !cur_canon.starts_with(root_canon);
            if outside {
                anyhow::bail!("{} is outside the workspace", candidate.display());
            }
        }
        candidate.clone()
    };

    if !resolved.starts_with(root) && !resolved.starts_with(root_canon) {
        anyhow::bail!("{} is outside the workspace", candidate.display());
    }

    Ok(candidate)
}

/// Whether a location's file lies outside the checkout at `root` (a path the client cannot
/// open itself).
pub fn is_external(root: &Path, file_path: &str) -> bool {
    let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path = Path::new(file_path);
    if !path.is_absolute() {
        return false;
    }
    let path_canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    !path.starts_with(&root) && !path_canon.starts_with(&root)
}

/// `file://` URI or plain path to a plain path. Only a URI is percent-decoded: a plain path is
/// already decoded, and a file named `100%41.rs` must not turn into `100A.rs`.
pub fn uri_to_path(uri: &str) -> String {
    prod_code_protocol::path::uri_or_path(uri)
        .to_string_lossy()
        .into_owned()
}

/// Lines `line - context ..= line + context` of `text` (1-based `line`), numbered, with the
/// target line marked.
pub fn snippet(text: &str, line: u32, context: u32) -> String {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return String::new();
    }
    let target = (line.max(1) as usize).min(lines.len());
    let from = target.saturating_sub(context as usize).max(1);
    let to = (target + context as usize).min(lines.len());
    let width = to.to_string().len();
    let mut out = String::new();
    for (idx, text) in lines.iter().enumerate().take(to).skip(from - 1) {
        let n = idx + 1;
        let marker = if n == target { ">" } else { " " };
        out.push_str(&format!("{marker}{n:>width$} | {text}\n"));
    }
    out
}

#[cfg(test)]
mod tests;
