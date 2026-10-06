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
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::diagnostics::stream::disk::{stream_session_dir, stream_session_disk_path};
use crate::diagnostics::stream::types::{MAX_STREAM_TOMBSTONES, StreamSession, StreamSessionKey};

/// Global manager for stateful streaming validation sessions (Roadmap 7.7).
/// Allows external producers (CLI line-by-line stdin pipe, MCP stream calls) to feed incremental chunks,
/// evaluate syntax and type checks at syntactic checkpoints, and interrupt generation on-the-fly.
pub struct StreamSessionManager {
    pub(crate) sessions: Mutex<HashMap<StreamSessionKey, StreamSession>>,
    pub(crate) tombstones: Mutex<HashMap<StreamSessionKey, Instant>>,
}

static STREAM_MANAGER: OnceLock<StreamSessionManager> = OnceLock::new();
static BATCH_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn stream_manager() -> &'static StreamSessionManager {
    STREAM_MANAGER.get_or_init(StreamSessionManager::new)
}

pub fn next_batch_counter() -> u64 {
    BATCH_COUNTER.fetch_add(1, Ordering::Relaxed)
}

impl Default for StreamSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamSessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            tombstones: Mutex::new(HashMap::new()),
        }
    }

    pub fn reset_session(&self, remote: SocketAddr, root: &Path, file: &Path, session_id: &str) {
        let key = StreamSessionKey::new(remote, root, file, session_id);
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(&key);
        }
        if let Ok(mut tombstones) = self.tombstones.lock() {
            tombstones.remove(&key);
        }
        let _ = std::fs::remove_file(stream_session_disk_path(&key));
    }

    pub(crate) fn insert_tombstone(
        tombstones: &mut HashMap<StreamSessionKey, Instant>,
        key: StreamSessionKey,
    ) {
        if !tombstones.contains_key(&key) && tombstones.len() >= MAX_STREAM_TOMBSTONES {
            if let Some(oldest) = tombstones
                .iter()
                .min_by_key(|(_, t)| *t)
                .map(|(k, _)| k.clone())
            {
                tombstones.remove(&oldest);
            }
        }
        tombstones.insert(key, Instant::now());
    }

    pub fn tombstone_count(&self) -> usize {
        self.tombstones.lock().map(|t| t.len()).unwrap_or(0)
    }

    pub(crate) fn remove_session_key(&self, key: &StreamSessionKey) {
        if let Ok(mut map) = self.sessions.lock() {
            map.remove(key);
        }
        if let Ok(mut tombstones) = self.tombstones.lock() {
            Self::insert_tombstone(&mut tombstones, key.clone());
        }
        let _ = std::fs::remove_file(stream_session_disk_path(key));
    }

    pub fn prune_stale(&self, max_age: std::time::Duration) {
        let now = Instant::now();
        if let Ok(mut map) = self.sessions.lock() {
            let mut expired = Vec::new();
            for (k, s) in map.iter() {
                if now.duration_since(s.last_activity) >= max_age {
                    expired.push(k.clone());
                }
            }
            if !expired.is_empty() {
                for k in &expired {
                    map.remove(k);
                    let _ = std::fs::remove_file(stream_session_disk_path(k));
                }
                if let Ok(mut tombstones) = self.tombstones.lock() {
                    for k in expired {
                        Self::insert_tombstone(&mut tombstones, k);
                    }
                }
            }
        }
        if let Ok(mut tombstones) = self.tombstones.lock() {
            tombstones.retain(|_, t| now.duration_since(*t) < std::time::Duration::from_secs(900));
        }
        let dir = stream_session_dir();
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("session") {
                    if let Ok(meta) = std::fs::metadata(&path) {
                        if let Ok(modified) = meta.modified() {
                            if let Ok(age) = std::time::SystemTime::now().duration_since(modified) {
                                if age
                                    >= std::cmp::max(max_age, std::time::Duration::from_secs(300))
                                {
                                    let _ = std::fs::remove_file(&path);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
