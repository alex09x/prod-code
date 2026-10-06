/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Diagnostics query execution and parallel function body inference priming.

use anyhow::Result;
use ra_ap_ide::{FileId, FileRange, FileStructureConfig, StructureNodeKind, SymbolKind};
use std::path::Path;

use super::RustEngine;
use crate::priming::PrimingJob;
use crate::types::FileDiagnostic;

impl RustEngine {
    pub fn diagnostics(&self, path: &Path) -> Result<Vec<FileDiagnostic>> {
        let started = std::time::Instant::now();
        self.infer_functions_in_parallel(path);
        let primed = started.elapsed();
        let result = self.snapshot_for_path(path).diagnostics(path);
        tracing::debug!(
            file = %path.display(),
            primed_ms = primed.as_millis() as u64,
            diagnostics_ms = (started.elapsed() - primed).as_millis() as u64,
            "diagnostics: functions inferred in parallel, then the diagnostics pass"
        );
        result
    }

    /// Type-checks the functions of `path` on several threads before its diagnostics are asked
    /// for (#86).
    ///
    /// Diagnostics need every body in the file inferred, and rust-analyzer infers them one after
    /// another. After a change to what the crate declares — the usual case for a proposal a
    /// refactoring validates — every body must be inferred again, which for a large file is tens
    /// of seconds on one core of a machine that has dozens. Highlighting a range resolves every
    /// name in it, and so infers the bodies there; doing that for each function on its own
    /// snapshot lets Salsa compute those inferences side by side, and the diagnostics pass that
    /// follows finds them done. A function is inferred by one thread at a time, so a file that is
    /// one huge function gains nothing. The snapshots are used and dropped before this returns:
    /// a snapshot kept alive would block the next write to the database.
    fn infer_functions_in_parallel(&self, path: &Path) {
        // The diagnostics pass runs next under the caller's lock; the job only infers.
        let mut job = self.priming_job(&[path]);
        job.files.clear();
        // One function gains nothing from a thread of its own.
        if job.threads() >= 2 {
            job.run();
        }
    }

    /// The work of inferring every function of `paths` on several threads, with its snapshots
    /// already taken. Making it is quick and needs the engine; running it does not, so a caller
    /// behind a lock makes the job under the lock and runs it after letting go (#233). A write
    /// to the database cancels a running job, which then stops.
    pub fn priming_job(&self, paths: &[&Path]) -> PrimingJob {
        let analysis = self.host.analysis();
        let mut ranges: Vec<FileRange> = Vec::new();
        let mut files: Vec<FileId> = Vec::new();
        for path in paths {
            let Some(file_id) = self.file_id_for_path(path) else {
                continue;
            };
            files.push(file_id);
            let Ok(nodes) = analysis.file_structure(
                &FileStructureConfig {
                    exclude_locals: true,
                },
                file_id,
            ) else {
                continue;
            };
            ranges.extend(
                nodes
                    .iter()
                    .filter(|n| {
                        matches!(
                            n.kind,
                            StructureNodeKind::SymbolKind(
                                SymbolKind::Function | SymbolKind::Method
                            )
                        )
                    })
                    .map(|n| FileRange {
                        file_id,
                        range: n.node_range,
                    }),
            );
        }
        drop(analysis);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
            .min(16)
            .min(ranges.len());
        // Largest first, dealt out in turn, so no thread is left with all the big ones.
        ranges.sort_by_key(|r| std::cmp::Reverse(r.range.len()));
        let mut shares: Vec<Vec<FileRange>> = vec![Vec::new(); threads];
        for (i, range) in ranges.into_iter().enumerate() {
            shares[i % threads].push(range);
        }
        PrimingJob::new(
            shares
                .into_iter()
                .map(|share| (self.host.analysis(), share))
                .collect(),
            files
                .into_iter()
                .map(|file_id| (self.host.analysis(), file_id))
                .collect(),
        )
    }
}
