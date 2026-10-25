# Issue 1330: local dependency review repairs

Pinned acceptance base: `f6063f7399342f220c93a2013e8e99be82749633`.
Repair input: `801a0af13d726fdce784eb906823d1f6ce21bfaf`.
All earlier audit files remain unchanged.

## Before and after

`before.txt` records the exact sanitized focused command and observed exit 101.
Three tests failed before production edits. Each assertion records the detector,
public checkout placement, compatible-default route and Linux-only route result:

- A portable parent with a test-only import of its required local replacement
  lost the dependency's macOS constraint in both detection and routing.
- A matching version-specific portable replacement failed to override an
  Apple-only wildcard replacement; routing incorrectly refused Linux.
- A wrapper's shared dependency was visited under its own owner before another
  selected module reached it. Global directory deduplication lost the latter
  owner's imports/replacement expansion and its nested Apple-only dependency.

`source-hashes.json` records the red-run production and fixture hashes, final
hashes and log hashes. Both red-run production files match the repair input.
The final fixture adds cases after reproduction; it is not claimed to be the
byte-identical red-run fixture.

## Repair

`crates/prod-code-mcp/src/sync/engine/cgo.rs:75` deduplicates directory/owner
pairs, retaining the shared 1,000-directory and 4,000-file traversal budgets.
Test-file imports expand dependencies before skipping direct cgo inspection;
Darwin/iOS filename suffixes and build constraints still exclude inapplicable
test files. Independent child modules retain their boundary.

`crates/prod-code-mcp/src/sync/engine/local_go.rs:117` suppresses wildcard
replacements when an exact required-version replacement exists, regardless of
declaration order. Versioned remote replacement records also suppress an unused
local wildcard, without fetching or inspecting their remote target. Required
and imported local targets remain canonicalized within the scan root. Existing
per-owner module, manifest-record and byte bounds remain unchanged.

The bounded source inspection remains distinct from a full Go dependency solver.
No network/toolchain discovery, external dependency-tree reading, configuration
or topology changes, restarts, installs, publication or delegation were added.
Every changed source module retains its header and is below 400 lines.

## Verification

- `format.txt`: one scoped remote rustfmt invocation; terminal exit 0.
- `after.txt`: the required remote cluster unit acceptance; terminal exit 0,
  63 passed, 0 failed, 1,363 filtered out. Five review regression tests cover
  test-only imports, exclusions/direct cgo, replacement order/fallback/remote
  overrides, and shared dependency expansion under its selected owner. Existing
  wrapper/go.work, nested selected-module, compatible-default, required-engine,
  remembered-Mac alternatives, OS probe-error and pressure cases also passed.
- `lint.txt`: normalized complete diagnostics from the single strict crate lint
  invocation; CLI exit 1, compiler exit 101, 210 diagnostics in 53 files. The
  diagnostic set exactly matches the earlier local-dependency lint record;
  none names a changed file. `lint-baseline-files.json` verifies all 53 named
  files are byte-identical to the pinned base. Strict lint did not pass. No
  pristine-base rerun or broader suite is claimed.
- `tool-failures.txt`: exact failed discovery commands/errors and bounded
  fallbacks. No command started here was left running.

Compilation and test execution occurred on a real Linux build node. Gateway
platform/OS, engine and placement responses were deterministic loopback fakes;
these tests do not demonstrate real macOS execution. Module/source fixtures
were actual temporary contained files. Commands retain their arguments with
private endpoint/check-out values replaced by descriptive placeholders.
