/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use crate::sync::types::{MAX_FILE_SIZE, MAX_JSON_CONFIG_SIZE, MAX_LIBRARY_SIZE, SyncFileEntry};
use prod_code_protocol::{FileDelta, content_hash};
use std::path::Path;
use std::time::UNIX_EPOCH;

pub(crate) fn stable_hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

pub(crate) fn sync_file_entry(metadata: &std::fs::Metadata, content: &[u8]) -> SyncFileEntry {
    let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
    let duration = modified.duration_since(UNIX_EPOCH).unwrap_or_default();
    SyncFileEntry {
        mtime_sec: duration.as_secs(),
        mtime_nsec: duration.subsec_nanos(),
        size: metadata.len(),
        hash: content_hash(content),
    }
}

/// Whether a file of this size is sent at all: large files are datasets, not sources.
pub(crate) fn fits_sync(relative_path: &str, metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
        && metadata.len() <= size_limit(relative_path)
        && !(relative_path.ends_with(".json") && metadata.len() > MAX_JSON_CONFIG_SIZE)
}

/// The largest file of this kind that is sent: a library a build links may be large, anything
/// else over [`MAX_FILE_SIZE`] is data.
pub(crate) fn size_limit(relative_path: &str) -> u64 {
    let library = Path::new(relative_path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| matches!(e, "a" | "so" | "dylib" | "lib" | "o"));
    if library {
        MAX_LIBRARY_SIZE
    } else {
        MAX_FILE_SIZE
    }
}

/// `files` in the order given, cut into messages of at most `budget` bytes of content each; a
/// file larger than the budget travels alone. Always at least one batch, possibly empty: an
/// empty delta is still sent, and its answer tells whether the gateway's copy still exists.
pub(crate) fn sync_batches(files: Vec<FileDelta>, budget: usize) -> Vec<Vec<FileDelta>> {
    let mut batches = vec![Vec::new()];
    let mut size = 0usize;
    for file in files {
        let len = file.content.as_ref().map_or(0, Vec::len);
        let current = batches.last().expect("never empty");
        if !current.is_empty() && size + len > budget {
            batches.push(Vec::new());
            size = 0;
        }
        size += len;
        batches.last_mut().expect("never empty").push(file);
    }
    batches
}

pub(crate) fn is_executable(metadata: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}
