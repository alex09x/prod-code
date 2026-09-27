# code_slice completeness: commands and results

All commands ran on a Linux aarch64 build node through `prod-code exec`, from base
78fbb9e plus this change. Each log begins with the exact command and ends with its exit status.
Private paths are replaced with `<node workspace copy>` / `<home>`.

| Log | What | Exit |
| --- | --- | --- |
| `red-base.log` | new `tests/slice_completeness.rs` against the base `slice.rs`: 10 failed at assertions, 1 passed (the ordinary-empty-answers guard), 1 ignored (live). The reported case reads `slice of \`seed\`: 1 item(s), 27 bytes from 47 bytes of source (43% smaller)` | 101 |
| `validate-slice-rs.log` | `prod-code validate` of the proposed `slice.rs`: 0 analyzer errors. The `--compile` shadow check failed on missing `.rmeta` files of `prod-code-protocol` dependencies in the shadow copy, not on this file | 1, then 0 |
| `fmt.log` | `cargo fmt --all` with write-back (three runs) | 0 |
| `green-narrow.log` | lib `slice::` (13), `slice_completeness` (11 + 1 ignored), `slicing` (25), before the documentSymbol leniency fix below | 0 |
| `live-real-analyzer.log` | `live.sh`: a standalone gateway from this revision and the ignored live test. It failed: the real engine answers `pub mod config;` with an inverted range, and strict validation rejected the whole file | 101 |
| `live-real-analyzer-2.log` | after the fix, only symbols the slicer uses need valid ranges. The slice was right, but the first definition query timed out during the cold load. The report showed it as `INCOMPLETE`, and the test stopped retrying too early | 101 |
| `live-real-analyzer-3.log` | retries until complete: attempt 1 `INCOMPLETE` (cold load), attempt 2 `slice of \`seed\`: 3 item(s) ... (31% smaller)` | 0 |
| `fmt-check.log` | `cargo fmt --all -- --check` | 0 |
| `clippy.log` | `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `full-mcp-tests.log` | `cargo test -p prod-code-mcp`: 411 lib and all integration binaries passed. Ignored: 2 live tests that need a gateway | 0 |
