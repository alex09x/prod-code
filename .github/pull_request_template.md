## Problem

What was wrong or missing, with the evidence: the command that shows it and its output.

## Change

What this PR does and why this way. Note anything deliberately left out.

## Reproduce and verify

The exact commands a reader runs to see the problem on `main` and the fix on this branch,
with their output. Name the node kind generically (32-core Linux node, macOS node,
developer workstation).

```sh
# before

# after
```

## Measurements

Before/after numbers for latency, throughput, memory or correctness rates, with hardware
and workload.

## Checks (run on a build node, paste the result lines)

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test -p <touched crates>`
- [ ] scenario from the issue against a running gateway

Closes #
