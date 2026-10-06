/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Maximum size of a single stream chunk (1 MB).
pub const MAX_STREAM_CHUNK_BYTES: usize = 1024 * 1024;
/// Maximum accumulated source buffer allowed per stream session (16 MB).
pub const MAX_STREAM_SESSION_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of concurrently retained active stream sessions.
pub const MAX_ACTIVE_STREAM_SESSIONS: usize = 128;
/// Maximum number of tombstone entries tracked to prevent silent prefix loss.
pub const MAX_STREAM_TOMBSTONES: usize = 1024;

/// Unique key scoping a stream session to its gateway remote, canonical workspace root, canonical file, and session ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamSessionKey {
    pub remote: SocketAddr,
    pub root: PathBuf,
    pub file: PathBuf,
    pub session_id: String,
}

impl StreamSessionKey {
    pub fn new(remote: SocketAddr, root: &Path, file: &Path, session_id: &str) -> Self {
        let root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        let file = if file.is_absolute() {
            std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf())
        } else {
            std::fs::canonicalize(root.join(file)).unwrap_or_else(|_| root.join(file))
        };
        Self {
            remote,
            root,
            file,
            session_id: session_id.to_string(),
        }
    }
}

/// Stateful stream validation session (Roadmap 7.7).
#[derive(Debug, Clone)]
pub struct StreamSession {
    pub key: StreamSessionKey,
    pub accumulated: String,
    pub chunk_count: usize,
    pub last_activity: Instant,
}
