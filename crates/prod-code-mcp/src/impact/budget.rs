/*
 * prod-code — Remote code intelligence
 * Copyright (c) 2026 Alexander Panasenko
 *
 * Contact: alex@prod.codes
 * Author: https://prod.codes/about/
 * Project: https://github.com/alex09x/prod-code
 * SPDX-License-Identifier: MIT OR Apache-2.0
 */

use super::types::Gap;

/// How long an impact analysis may take unless the caller asks for more.
pub(crate) const IMPACT_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

/// The most a caller may ask for: a language server of another language than the project's
/// can need minutes to read a project cold, and an unbounded wait is not a budget (#1960).
pub(crate) const MAX_IMPACT_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

/// The budget a `budget_secs` argument names: the default when absent, otherwise 1 to
/// [`MAX_IMPACT_BUDGET`] seconds; anything else is refused rather than read as the default.
pub fn budget_from_secs(secs: Option<u64>) -> anyhow::Result<std::time::Duration> {
    let Some(secs) = secs else {
        return Ok(IMPACT_BUDGET);
    };
    anyhow::ensure!(
        (1..=MAX_IMPACT_BUDGET.as_secs()).contains(&secs),
        "`budget_secs` is {secs}; it must be between 1 and {} seconds",
        MAX_IMPACT_BUDGET.as_secs()
    );
    Ok(std::time::Duration::from_secs(secs))
}

pub(crate) async fn within_deadline<T>(
    deadline: tokio::time::Instant,
    future: impl std::future::Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    tokio::time::timeout_at(deadline, future).await
}

pub(crate) fn record_timeout(
    started_at: tokio::time::Instant,
    budget: std::time::Duration,
    incomplete: &mut Vec<Gap>,
) {
    if incomplete
        .iter()
        .any(|gap| matches!(gap, Gap::Timeout { .. }))
    {
        return;
    }
    super::incoming::note(
        incomplete,
        Gap::Timeout {
            elapsed_ms: started_at.elapsed().as_millis() as u64,
            budget_ms: budget.as_millis() as u64,
        },
    );
}

pub(crate) fn expired(
    started_at: tokio::time::Instant,
    deadline: tokio::time::Instant,
    incomplete: &mut Vec<Gap>,
) -> bool {
    if tokio::time::Instant::now() < deadline {
        return false;
    }
    record_timeout(
        started_at,
        deadline.saturating_duration_since(started_at),
        incomplete,
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_budget_is_reported_once() {
        let started_at = tokio::time::Instant::now() - IMPACT_BUDGET;
        let deadline = started_at + IMPACT_BUDGET;
        let mut incomplete = Vec::new();

        assert!(expired(started_at, deadline, &mut incomplete));
        assert!(expired(started_at, deadline, &mut incomplete));
        assert_eq!(incomplete.len(), 1);
        assert!(matches!(incomplete[0], Gap::Timeout { .. }));
    }

    #[test]
    fn timeout_from_an_expired_operation_is_recorded_even_before_the_clock_tick() {
        let started_at = tokio::time::Instant::now();
        let mut incomplete = Vec::new();

        record_timeout(started_at, IMPACT_BUDGET, &mut incomplete);

        assert_eq!(incomplete.len(), 1);
        assert!(matches!(incomplete[0], Gap::Timeout { .. }));
    }

    #[test]
    fn a_budget_argument_is_the_default_or_a_bounded_number_of_seconds() {
        assert_eq!(budget_from_secs(None).unwrap(), IMPACT_BUDGET);
        assert_eq!(
            budget_from_secs(Some(90)).unwrap(),
            std::time::Duration::from_secs(90)
        );
        assert_eq!(budget_from_secs(Some(1)).unwrap().as_secs(), 1);
        assert_eq!(budget_from_secs(Some(600)).unwrap().as_secs(), 600);
        for bad in [0, 601, u64::MAX] {
            let error = budget_from_secs(Some(bad)).unwrap_err().to_string();
            assert!(error.contains("between 1 and 600"), "{error}");
        }
    }

    /// The gap names the budget the analysis was given, not the default one (#1960).
    #[test]
    fn the_timeout_gap_reports_the_budget_it_was_given() {
        let started_at = tokio::time::Instant::now() - std::time::Duration::from_secs(90);
        let budget = std::time::Duration::from_secs(60);
        let mut incomplete = Vec::new();

        assert!(expired(started_at, started_at + budget, &mut incomplete));

        assert!(
            matches!(
                incomplete[0],
                Gap::Timeout {
                    budget_ms: 60_000,
                    ..
                }
            ),
            "{incomplete:?}"
        );
    }

    #[tokio::test]
    async fn within_deadline_stops_an_operation_that_exceeds_the_budget() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1);
        let result = within_deadline(deadline, tokio::time::sleep(IMPACT_BUDGET)).await;

        assert!(result.is_err());
    }
}
