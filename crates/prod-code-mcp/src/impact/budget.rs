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

pub(crate) const IMPACT_BUDGET: std::time::Duration = std::time::Duration::from_secs(15);

pub(crate) async fn within_deadline<T>(
    deadline: tokio::time::Instant,
    future: impl std::future::Future<Output = T>,
) -> Result<T, tokio::time::error::Elapsed> {
    tokio::time::timeout_at(deadline, future).await
}

pub(crate) fn record_timeout(started_at: tokio::time::Instant, incomplete: &mut Vec<Gap>) {
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
            budget_ms: IMPACT_BUDGET.as_millis() as u64,
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
    record_timeout(started_at, incomplete);
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

        record_timeout(started_at, &mut incomplete);

        assert_eq!(incomplete.len(), 1);
        assert!(matches!(incomplete[0], Gap::Timeout { .. }));
    }

    #[tokio::test]
    async fn within_deadline_stops_an_operation_that_exceeds_the_budget() {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(1);
        let result = within_deadline(deadline, tokio::time::sleep(IMPACT_BUDGET)).await;

        assert!(result.is_err());
    }
}
