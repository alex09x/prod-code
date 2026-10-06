/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::{Expectations, QueryOutcome, VerificationResult, WorktreeKind};

type HoverPredicate<'a> = Box<dyn Fn(&str) -> bool + 'a>;

/// Groups query outcomes by worktree kind and asserts each kind's expected correctness
/// invariant, catching any cross-worktree bleed.
pub fn verify(outcomes: &[QueryOutcome], expect: &Expectations) -> Vec<VerificationResult> {
    let symbol = expect.symbol.as_str();
    let marker = expect.marker.as_str();
    let untracked = expect.untracked_symbol.as_str();

    WorktreeKind::all()
        .into_iter()
        .map(|kind| {
            let relevant: Vec<&QueryOutcome> =
                outcomes.iter().filter(|o| o.kind == kind && o.ok).collect();

            if relevant.is_empty() {
                let sample = outcomes
                    .iter()
                    .find(|o| o.kind == kind)
                    .map(|o| o.detail.clone())
                    .unwrap_or_default();
                return VerificationResult {
                    kind,
                    passed: false,
                    message: format!("no successful responses for {}", kind.label()),
                    violations: 0,
                    sample,
                };
            }

            let (predicate, message): (HoverPredicate, &str) = match kind {
                WorktreeKind::Master => (
                    Box::new(|d: &str| d.contains(symbol) && !d.contains(marker)),
                    "master must show the unmutated base signature with no marker bleed from worktree A",
                ),
                WorktreeKind::SignatureChange => (
                    Box::new(|d: &str| d.contains(symbol) && d.contains(marker)),
                    "worktree A must show the mutated signature carrying the marker parameter",
                ),
                WorktreeKind::DependencyChange => (
                    Box::new(|d: &str| d.contains(symbol) && !d.contains(marker)),
                    "worktree B only touches the manifest; the signature must stay in base form",
                ),
                WorktreeKind::UntrackedFile => (
                    Box::new(|d: &str| d.contains(untracked)),
                    "worktree C must resolve the symbol from its untracked file",
                ),
            };

            let violating: Vec<&&QueryOutcome> =
                relevant.iter().filter(|o| !predicate(&o.detail)).collect();
            let sample = violating
                .first()
                .or(relevant.first().as_ref().map(|o| *o).as_ref())
                .map(|o| o.detail.clone())
                .unwrap_or_default();

            VerificationResult {
                kind,
                passed: violating.is_empty(),
                message: message.to_string(),
                violations: violating.len(),
                sample,
            }
        })
        .collect()
}
