# code_slice completeness: commands and results

All commands run on a Linux aarch64 build node through `prod-code exec`. Each is repeatable
as written from the repository root.

## Evidence, bounds and coordinates

`crates/prod-code-mcp/tests/slice_completeness.rs` drives the slicer through a scripted
gateway. Against the slicer of the previous revision, four new tests and the updated
depth/budget test failed at their assertions (10 passed, 5 failed, exit 101):

- a definition URI that is empty (`""`), relative (`src/lib.rs`), `file:src/lib.rs`, a file
  URI of another host, or a jar (`jdt://...`) was listed as `outside the workspace, not
  followed`, and a file URI with `?version=2` was followed as that file;
- a definition position past the last line, past the end of a line, inside a surrogate pair,
  or between the `\r` and `\n` of a CRLF line was followed to whatever declaration spans that
  line, and a declaration running past its file was sliced from the lines that exist;
- a seed column of 500 on an 11-unit line returned the declaration spanning the line;
- a slice cut by the depth limit, the byte budget, or a target outside every sliced
  declaration read `slice of ...` and `is_complete()` was true.

With the change these are malformed evidence (named, `INCOMPLETE`), unsupported external
sources (`name (jdt:)`), a refused seed, and `BOUNDED` slices; `has_complete_evidence()` keeps
the answered-every-lookup meaning that `is_complete()` had.

```sh
prod-code exec --timeout-secs 900 --no-pull -- cargo test -p prod-code-mcp --test slice_completeness
prod-code exec --timeout-secs 900 --no-pull -- cargo test -p prod-code-mcp
```

## A real engine through the public tool

`crates/prod-code-gateway/tests/slice_live.rs` starts the `prod-code-server` built by the same
`cargo test`, bound to port 0 (the test reads the address the daemon reports), syncs a small
crate to it and calls `code_slice`: a complete three-item slice across two files, the same
from a line-only seed, a complete slice of a CRLF file with a call after `é😀`, a `BOUNDED`
slice at depth 0, and a refused column 200 on line 6. No address or port is configured, and
nothing is skipped.

```sh
prod-code exec --timeout-secs 1800 --no-pull -- cargo test -p prod-code-gateway --test slice_live -- --nocapture
```

## Checks

```sh
prod-code exec -- cargo fmt --all -- --check
prod-code exec --timeout-secs 1800 --no-pull -- cargo clippy --workspace --all-targets -- -D warnings
prod-code exec --timeout-secs 1800 --no-pull -- bash -lc 'set -o pipefail; cargo llvm-cov -p prod-code-mcp --json --output-path target/cov-mcp.json && python3 scripts/coverage.py --report target/cov-mcp.json --min 80 crates/prod-code-mcp/src/slice.rs'
```

Results for this change: `prod-code-mcp` tests all passed (lib 414; `slice_completeness` 15;
`slicing` 25; `tools` 104; 1 test ignored in `parameter_object_drop`, unrelated), the live
test passed (first answer `INCOMPLETE` during the cold load, complete on the second), fmt and
clippy clean, and `slice.rs` at 97.36% of regions (1879/1930).
