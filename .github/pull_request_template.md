## Problem

What was wrong or missing, with the evidence (a measurement, a failing case, a log line).

## Change

What this PR does and why this way. Note anything deliberately left out.

## Measurements

Before/after numbers where the change is about latency, throughput, memory or correctness rates. Say on what hardware and workload they were taken.

## Checks

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] tests for the touched crates
- [ ] verified against a running gateway (which scenario)

Closes #
