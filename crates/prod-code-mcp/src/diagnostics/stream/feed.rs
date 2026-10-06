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
use std::net::SocketAddr;
use std::path::Path;
use std::time::Instant;

use crate::diagnostics::stream::disk::{stream_session_disk_path, write_private_session_file};
use crate::diagnostics::stream::manager::StreamSessionManager;
use crate::diagnostics::stream::types::{
    MAX_ACTIVE_STREAM_SESSIONS, MAX_STREAM_CHUNK_BYTES, MAX_STREAM_SESSION_BYTES, StreamSession,
    StreamSessionKey,
};
use crate::diagnostics::types::{HallucinationInterception, HallucinationKind, StreamChunkResult};
use crate::diagnostics::validate::single::validate_text;

impl StreamSessionManager {
    /// Feed a chunk into a stateful streaming session and validate incrementally.
    pub async fn feed_chunk(
        &self,
        remote: SocketAddr,
        root: &Path,
        file: &Path,
        session_id: &str,
        chunk: &str,
        is_last: bool,
        reset: bool,
        borrow_check: bool,
    ) -> Result<StreamChunkResult> {
        if chunk.len() > MAX_STREAM_CHUNK_BYTES {
            anyhow::bail!(
                "stream chunk size ({} bytes) exceeds maximum limit of {} bytes",
                chunk.len(),
                MAX_STREAM_CHUNK_BYTES
            );
        }

        self.prune_stale(std::time::Duration::from_secs(300));
        let key = StreamSessionKey::new(remote, root, file, session_id);

        let (accumulated, chunk_index) = {
            let mut map = self.sessions.lock().unwrap();

            if reset {
                map.remove(&key);
                if let Ok(mut tombstones) = self.tombstones.lock() {
                    tombstones.remove(&key);
                }
                let _ = std::fs::remove_file(stream_session_disk_path(&key));
            } else {
                let is_tombstone = self
                    .tombstones
                    .lock()
                    .map(|t| t.contains_key(&key))
                    .unwrap_or(false);
                if is_tombstone {
                    let _ = std::fs::remove_file(stream_session_disk_path(&key));
                    anyhow::bail!(
                        "stream session '{}' has expired or was terminated; pass reset: true to start a new stream",
                        session_id
                    );
                }

                if !map.contains_key(&key) {
                    let disk_path = stream_session_disk_path(&key);
                    let mut restored = false;
                    if disk_path.is_file() {
                        let is_expired = if let Ok(meta) = std::fs::metadata(&disk_path) {
                            if let Ok(modified) = meta.modified() {
                                std::time::SystemTime::now()
                                    .duration_since(modified)
                                    .map(|d| d >= std::time::Duration::from_secs(300))
                                    .unwrap_or(false)
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if is_expired {
                            let _ = std::fs::remove_file(&disk_path);
                            if let Ok(mut tombstones) = self.tombstones.lock() {
                                Self::insert_tombstone(&mut tombstones, key.clone());
                            }
                            anyhow::bail!(
                                "stream session '{}' has expired or was terminated; pass reset: true to start a new stream",
                                session_id
                            );
                        }

                        if map.len() >= MAX_ACTIVE_STREAM_SESSIONS {
                            anyhow::bail!(
                                "maximum concurrent active stream sessions limit ({}) reached; retry later or reset unused sessions",
                                MAX_ACTIVE_STREAM_SESSIONS
                            );
                        }

                        if let Ok(data) = std::fs::read_to_string(&disk_path) {
                            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&data) {
                                if let (Some(acc), Some(count)) =
                                    (val["accumulated"].as_str(), val["chunk_count"].as_u64())
                                {
                                    map.insert(
                                        key.clone(),
                                        StreamSession {
                                            key: key.clone(),
                                            accumulated: acc.to_string(),
                                            chunk_count: count as usize,
                                            last_activity: Instant::now(),
                                        },
                                    );
                                    restored = true;
                                }
                            }
                        }
                    }

                    if !restored {
                        anyhow::bail!(
                            "stream session '{}' not found or was evicted; pass reset: true to start a new stream",
                            session_id
                        );
                    }
                }
            }

            if !map.contains_key(&key) {
                if map.len() >= MAX_ACTIVE_STREAM_SESSIONS {
                    anyhow::bail!(
                        "maximum concurrent active stream sessions limit ({}) reached; retry later or reset unused sessions",
                        MAX_ACTIVE_STREAM_SESSIONS
                    );
                }
            }

            let current_len = map.get(&key).map(|s| s.accumulated.len()).unwrap_or(0);
            if current_len + chunk.len() > MAX_STREAM_SESSION_BYTES {
                map.remove(&key);
                if let Ok(mut tombstones) = self.tombstones.lock() {
                    Self::insert_tombstone(&mut tombstones, key.clone());
                }
                let _ = std::fs::remove_file(stream_session_disk_path(&key));
                anyhow::bail!(
                    "stream session accumulated buffer ({} bytes) exceeds maximum limit of {} bytes",
                    current_len + chunk.len(),
                    MAX_STREAM_SESSION_BYTES
                );
            }

            let session = map.entry(key.clone()).or_insert_with(|| StreamSession {
                key: key.clone(),
                accumulated: String::new(),
                chunk_count: 0,
                last_activity: Instant::now(),
            });

            session.accumulated.push_str(chunk);
            session.chunk_count += 1;
            session.last_activity = Instant::now();

            let disk_path = stream_session_disk_path(&key);
            let payload = serde_json::json!({
                "accumulated": &session.accumulated,
                "chunk_count": session.chunk_count,
            });
            let _ = write_private_session_file(&disk_path, &payload.to_string());

            (session.accumulated.clone(), session.chunk_count)
        };

        let accumulated_bytes = accumulated.len();

        let is_checkpoint =
            is_last || chunk.contains('\n') || chunk.contains(';') || chunk.contains('}');

        if !is_checkpoint {
            return Ok(StreamChunkResult {
                session_id: session_id.to_string(),
                chunk_index,
                accumulated_bytes,
                checkpoint_evaluated: false,
                intercepted: false,
                interception: None,
                final_report: None,
                summary: format!(
                    "buffered chunk {} ({} bytes)",
                    chunk_index, accumulated_bytes
                ),
            });
        }

        match validate_text(remote, root, file, &accumulated).await {
            Ok(report) => {
                // A later chunk can add imports, trait impls, declarations, and types that make
                // semantic errors in this prefix valid. Do not terminate generation until the
                // source unit is complete.
                let serious_intercept = if is_last {
                    report.hallucinations.first().cloned()
                } else {
                    None
                };

                if let Some(intercept) = serious_intercept {
                    self.remove_session_key(&key);
                    return Ok(StreamChunkResult {
                        session_id: session_id.to_string(),
                        chunk_index,
                        accumulated_bytes,
                        checkpoint_evaluated: true,
                        intercepted: true,
                        interception: Some(intercept.clone()),
                        final_report: Some(report),
                        summary: format!(
                            "intercepted {} at chunk {} (line {}:{})",
                            intercept.kind, chunk_index, intercept.line, intercept.col
                        ),
                    });
                }

                if is_last {
                    self.remove_session_key(&key);
                    let errors = report.errors;
                    if borrow_check && errors == 0 {
                        let (compiled_errors, compiled_out) = crate::tools::compile_check(
                            remote,
                            root,
                            &[(file.to_path_buf(), accumulated.clone())],
                        )
                        .await?;
                        if compiled_errors > 0 {
                            return Ok(StreamChunkResult {
                                session_id: session_id.to_string(),
                                chunk_index,
                                accumulated_bytes,
                                checkpoint_evaluated: true,
                                intercepted: true,
                                interception: Some(HallucinationInterception {
                                    kind: HallucinationKind::BorrowCheckerError,
                                    symbol_or_target: None,
                                    message: compiled_out,
                                    line: 0,
                                    col: 0,
                                    suggestion: Some(
                                        "Address compiler/borrow-checker violations".to_string(),
                                    ),
                                }),
                                final_report: Some(report),
                                summary: "borrow-checker / compiler verification rejected proposal"
                                    .to_string(),
                            });
                        }
                    }

                    let passed = errors == 0;
                    return Ok(StreamChunkResult {
                        session_id: session_id.to_string(),
                        chunk_index,
                        accumulated_bytes,
                        checkpoint_evaluated: true,
                        intercepted: !passed,
                        interception: report.hallucinations.first().cloned(),
                        final_report: Some(report),
                        summary: if passed {
                            "stream generation validated clean: 0 hallucinations intercepted"
                                .to_string()
                        } else {
                            format!("stream completed with {errors} error(s)")
                        },
                    });
                }

                Ok(StreamChunkResult {
                    session_id: session_id.to_string(),
                    chunk_index,
                    accumulated_bytes,
                    checkpoint_evaluated: true,
                    intercepted: false,
                    interception: None,
                    final_report: None,
                    summary: if report.errors > 0 {
                        format!(
                            "checkpoint at chunk {} found {} provisional error(s); semantic interceptions are deferred until close",
                            chunk_index, report.errors
                        )
                    } else {
                        format!("checkpoint at chunk {} passed", chunk_index)
                    },
                })
            }
            Err(err) => {
                if is_last {
                    self.remove_session_key(&key);
                    Err(err)
                } else {
                    Ok(StreamChunkResult {
                        session_id: session_id.to_string(),
                        chunk_index,
                        accumulated_bytes,
                        checkpoint_evaluated: true,
                        intercepted: false,
                        interception: None,
                        final_report: None,
                        summary: format!(
                            "checkpoint at chunk {} skipped due to intermediate syntax: {err}",
                            chunk_index
                        ),
                    })
                }
            }
        }
    }
}
