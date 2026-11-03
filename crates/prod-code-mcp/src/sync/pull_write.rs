/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Writing a file the node sent back into the checkout: confined to the checkout, regular files
//! only, replaced atomically, with the permissions the caller's policy asks for (#1823).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Whose executable bit an existing local file ends up with when the node's version replaces it.
/// A new file always takes the node's bit, as far as the local umask allows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalMode {
    /// An explicit pull: the node's executable bit wins both ways, other bits stay (#1477).
    FollowNode,
    /// Exec write-back: the local permission bits stay exactly as they were. The node copy is a
    /// mirror whose mode may be stale (an older client sent it without the bit), so a formatter
    /// rewriting a script must not reset the local 0755 or 0640 (#1823).
    Preserve,
}

/// The error kind of a refused target, which a caller skips instead of failing the batch.
pub(crate) const REFUSED: std::io::ErrorKind = std::io::ErrorKind::InvalidInput;

fn refused(why: &str) -> std::io::Error {
    std::io::Error::new(REFUSED, format!("pulled file refused: {why}"))
}

/// Replaces `root/rel` with `content` through a temporary sibling and a rename, so a failed
/// write leaves the old file, and returns the path written. `root` must be canonical and `rel`
/// free of `..` and absolute parts. A contained symlink to a regular file has its target
/// replaced, the link is kept; a link out of the checkout, a dangling link, a directory or any
/// other special file is refused, and so is a parent directory that resolves outside `root`.
/// Only the permission bits 0o777 are ever set: no setuid, setgid or sticky bit, no owner.
pub(crate) fn replace_pulled_file(
    root: &Path,
    rel: &Path,
    content: &[u8],
    node_executable: bool,
    policy: LocalMode,
) -> std::io::Result<PathBuf> {
    let target = root.join(rel);
    let parent = target
        .parent()
        .ok_or_else(|| refused("no parent directory"))?;
    ensure_contained_dir(root, parent)?;
    let (dest, existing) = match std::fs::symlink_metadata(&target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let resolved =
                std::fs::canonicalize(&target).map_err(|_| refused("dangling symlink"))?;
            if !resolved.starts_with(root) {
                return Err(refused("symlink points outside the checkout"));
            }
            let meta = std::fs::metadata(&resolved)?;
            if !meta.is_file() {
                return Err(refused("symlink target is not a regular file"));
            }
            (resolved, Some(meta))
        }
        Ok(meta) if meta.is_file() => (target, Some(meta)),
        Ok(_) => return Err(refused("not a regular file")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (target, None),
        Err(e) => return Err(e),
    };
    let permissions = permission_bits(existing.as_ref(), node_executable, policy);
    write_and_swap(&dest, content, node_executable, permissions)?;
    Ok(dest)
}

/// The directory a pulled file goes into, created if needed, must resolve inside `root`:
/// a symlinked parent must not carry the write, or the directories made for it, elsewhere.
fn ensure_contained_dir(root: &Path, dir: &Path) -> std::io::Result<()> {
    let mut existing = dir;
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing
            .parent()
            .ok_or_else(|| refused("no existing ancestor"))?;
    }
    let contained = |path: &Path| {
        std::fs::canonicalize(path).is_ok_and(|real| real.starts_with(root) && real.is_dir())
    };
    if !contained(existing) {
        return Err(refused("parent directory resolves outside the checkout"));
    }
    std::fs::create_dir_all(dir)?;
    if !contained(dir) {
        return Err(refused("parent directory resolves outside the checkout"));
    }
    Ok(())
}

/// The exact bits to give the replacement, or `None` for a new file, which is created with
/// 0o777 or 0o666 and so takes the local umask.
#[cfg(unix)]
fn permission_bits(
    existing: Option<&std::fs::Metadata>,
    node_executable: bool,
    policy: LocalMode,
) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    let base = existing?.permissions().mode() & 0o777;
    Some(match policy {
        LocalMode::Preserve => base,
        LocalMode::FollowNode if node_executable => 0o755,
        LocalMode::FollowNode => base & !0o111,
    })
}

#[cfg(not(unix))]
fn permission_bits(_: Option<&std::fs::Metadata>, _: bool, _: LocalMode) -> Option<u32> {
    None
}

fn write_and_swap(
    dest: &Path,
    content: &[u8],
    node_executable: bool,
    permissions: Option<u32>,
) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let name = dest
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let temp = dest.with_file_name(format!(
        ".{name}.prod-code-pull-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let create_mode = match permissions {
            Some(_) => 0o600,
            None if node_executable => 0o777,
            None => 0o666,
        };
        options.mode(create_mode);
    }
    #[cfg(not(unix))]
    let _ = (node_executable, permissions);
    let written = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(content)?;
        #[cfg(unix)]
        if let Some(bits) = permissions {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(bits))?;
        }
        drop(file);
        std::fs::rename(&temp, dest)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}
