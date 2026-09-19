//! Shared benchmarking primitives for prod-code-client.
//!
//! This is the library crate root for `prod-code-client`: the `prod-code` binary (`src/main.rs`)
//! pulls in [`divergent_bench`] to implement the `divergent-bench` subcommand, while keeping its
//! own inline single-workspace pipelined benchmark untouched.

pub mod divergent_bench;

/// Latency percentile summary (in milliseconds) computed from a set of microsecond samples.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LatencyStats {
    pub count: usize,
    pub min_ms: f64,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl LatencyStats {
    /// Sorts `latencies_us` in place and derives min/p50/p95/p99/max in milliseconds.
    pub fn from_micros(latencies_us: &mut [u64]) -> Self {
        if latencies_us.is_empty() {
            return Self::default();
        }
        latencies_us.sort_unstable();
        let to_ms = |us: u64| us as f64 / 1000.0;
        Self {
            count: latencies_us.len(),
            min_ms: to_ms(latencies_us[0]),
            p50_ms: to_ms(percentile_us(latencies_us, 50.0)),
            p95_ms: to_ms(percentile_us(latencies_us, 95.0)),
            p99_ms: to_ms(percentile_us(latencies_us, 99.0)),
            max_ms: to_ms(latencies_us[latencies_us.len() - 1]),
        }
    }
}

/// Nearest-rank percentile over an ascending-sorted slice of microsecond latencies.
pub fn percentile_us(sorted_us: &[u64], pct: f64) -> u64 {
    if sorted_us.is_empty() {
        return 0;
    }
    let rank = ((pct / 100.0) * sorted_us.len() as f64).ceil() as usize;
    let idx = rank.saturating_sub(1).min(sorted_us.len() - 1);
    sorted_us[idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_of_empty_is_zero() {
        assert_eq!(percentile_us(&[], 50.0), 0);
    }

    #[test]
    fn percentile_matches_nearest_rank() {
        let sorted: Vec<u64> = (1..=100).collect();
        assert_eq!(percentile_us(&sorted, 50.0), 50);
        assert_eq!(percentile_us(&sorted, 95.0), 95);
        assert_eq!(percentile_us(&sorted, 99.0), 99);
        assert_eq!(percentile_us(&sorted, 100.0), 100);
    }

    #[test]
    fn latency_stats_from_micros() {
        let mut samples = vec![5000u64, 1000, 3000, 2000, 4000];
        let stats = LatencyStats::from_micros(&mut samples);
        assert_eq!(stats.count, 5);
        assert_eq!(stats.min_ms, 1.0);
        assert_eq!(stats.max_ms, 5.0);
        assert_eq!(stats.p50_ms, 3.0);
    }

    #[test]
    fn latency_stats_of_empty_is_default() {
        let mut samples: Vec<u64> = Vec::new();
        let stats = LatencyStats::from_micros(&mut samples);
        assert_eq!(stats, LatencyStats::default());
    }
}
