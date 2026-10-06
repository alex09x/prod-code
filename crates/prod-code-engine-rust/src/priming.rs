/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

//! Multi-threaded function body inference and diagnostics priming.

use ra_ap_ide::{
    Analysis, AssistResolveStrategy, DiagnosticsConfig, FileId, FileRange, HighlightConfig,
    RaFixtureConfig,
};

/// Functions to infer, dealt out to threads, each share with its own snapshot. Made by
/// [`RustEngine::priming_job`](crate::engine::RustEngine::priming_job); see there.
pub struct PrimingJob {
    pub(crate) shares: Vec<(Analysis, Vec<FileRange>)>,
    /// The files whose full diagnostics are computed after the inference, each on its own
    /// snapshot: rust-analyzer caches them, and a validation asks for exactly these.
    pub(crate) files: Vec<(Analysis, FileId)>,
}

impl PrimingJob {
    pub(crate) fn new(
        shares: Vec<(Analysis, Vec<FileRange>)>,
        files: Vec<(Analysis, FileId)>,
    ) -> Self {
        Self { shares, files }
    }

    /// How many threads the job runs on (0 when there is nothing to infer).
    pub fn threads(&self) -> usize {
        self.shares.len()
    }

    /// Infers the functions, one thread per share, then computes each file's full diagnostics,
    /// one thread per file. Highlighting a range resolves every name in it, and so infers the
    /// bodies there; the diagnostics pass that follows is most of what a validation costs cold
    /// (21 s of 22 for a 4,246-line file on a Linux build node, #233). Returns how many
    /// functions were inferred; a thread stops at its first error, which is a write to the
    /// database cancelling it. The snapshots are dropped before this returns: one kept alive
    /// would block the next write.
    pub fn run(self) -> usize {
        let files = self.files;
        let workers: Vec<_> = self
            .shares
            .into_iter()
            .map(|(analysis, share)| {
                std::thread::spawn(move || {
                    let config = HighlightConfig {
                        strings: false,
                        comments: false,
                        punctuation: false,
                        specialize_punctuation: false,
                        operator: false,
                        specialize_operator: false,
                        inject_doc_comment: false,
                        macro_bang: false,
                        syntactic_name_ref_highlighting: false,
                        ra_fixture: RaFixtureConfig::default(),
                    };
                    let mut done = 0;
                    for range in share {
                        if analysis.highlight_range(config, range).is_err() {
                            break;
                        }
                        done += 1;
                    }
                    done
                })
            })
            .collect();
        let inferred = workers.into_iter().map(|w| w.join().unwrap_or(0)).sum();
        let diagnosed: Vec<_> = files
            .into_iter()
            .map(|(analysis, file_id)| {
                std::thread::spawn(move || {
                    let started = std::time::Instant::now();
                    let outcome = analysis.full_diagnostics(
                        &DiagnosticsConfig::test_sample(),
                        AssistResolveStrategy::None,
                        file_id,
                    );
                    tracing::debug!(
                        ?file_id,
                        done = outcome.is_ok(),
                        ms = started.elapsed().as_millis() as u64,
                        "warmed the diagnostics of a file"
                    );
                })
            })
            .collect();
        for worker in diagnosed {
            let _ = worker.join();
        }
        inferred
    }
}
