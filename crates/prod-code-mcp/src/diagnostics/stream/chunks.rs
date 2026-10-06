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

use crate::diagnostics::stream::manager::{next_batch_counter, stream_manager};
use crate::diagnostics::types::StreamValidationResult;

/// Streamed syntax and type check verification during agent code generation (Phase 7.7).
/// Intercepts invalid method invocations, incorrect argument types, or borrow-checker errors
/// before the agent even finishes generating its turn, providing immediate feedback and
/// eliminating multi-turn debugging cycles.
pub async fn validate_stream_chunks(
    remote: SocketAddr,
    root: &Path,
    file: &Path,
    chunks: &[String],
    borrow_check: bool,
) -> Result<StreamValidationResult> {
    let count = next_batch_counter();
    let session_id = format!("batch-{}-{}", std::process::id(), count);
    let mgr = stream_manager();
    mgr.reset_session(remote, root, file, &session_id);
    let total_chunks = chunks.len();

    for (idx, chunk) in chunks.iter().enumerate() {
        let is_last = idx + 1 == total_chunks;
        let is_first = idx == 0;
        let res = mgr
            .feed_chunk(
                remote,
                root,
                file,
                &session_id,
                chunk,
                is_last,
                is_first,
                borrow_check,
            )
            .await?;
        if res.intercepted {
            mgr.reset_session(remote, root, file, &session_id);
            return Ok(StreamValidationResult {
                completed_chunks: idx + 1,
                total_chunks,
                intercepted: true,
                interception: res.interception,
                intercept_chunk_index: Some(idx),
                final_report: res.final_report,
                summary: res.summary,
            });
        }
        if is_last {
            mgr.reset_session(remote, root, file, &session_id);
            return Ok(StreamValidationResult {
                completed_chunks: total_chunks,
                total_chunks,
                intercepted: false,
                interception: None,
                intercept_chunk_index: None,
                final_report: res.final_report,
                summary: res.summary,
            });
        }
    }

    mgr.reset_session(remote, root, file, &session_id);
    Ok(StreamValidationResult {
        completed_chunks: total_chunks,
        total_chunks,
        intercepted: false,
        interception: None,
        intercept_chunk_index: None,
        final_report: None,
        summary: "stream completed".to_string(),
    })
}
