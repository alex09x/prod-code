# Issue 1316: caller source preservation and overlapping edits

This follow-up starts at `14599b9a1fbc8ee33e2afe1a6ce4f195fb300491`.
The aggregate acceptance base remains `1fadae441d74e155d219bef7829817c8f5c59575`.
All earlier audit evidence is unchanged. The original raw-owner, array-shift and
ordinary-caller pre-edit runtime observations remain in
`../1316-aggregate-repair/before-owner.txt`, `before.txt` and `controls-before.txt`.

## Observations before this production edit

`before.txt` records the public conversion entrypoint with semantic-response fixtures
and real rustc/runtime. The original comment fixture compiles and prints `3`;
conversion trims the comment's terminating newline and rustc rejects the result.
The original nested fixture compiles and prints `4`; conversion reports two rewrites
and emits corrupted Rust. This layout demonstrates corruption rather than the
out-of-bounds panic reported by the preceding review. Both tests fail, exit 101.
The pre-edit run stops at the first failing mode in each test; it does not claim
successful execution of every pre-edit mode.

## Repair and verification

The conversion preserves the original remaining-argument source span, including
newlines terminating line comments. Before rendering a dispatch-proof overlay, it
rejects a call edit overlapping an accepted edit. The overlapping call retains its
qualified spelling; which call converts follows the analyzer's reference order.
Ordinary independent exact-owner calls still convert using the existing type,
nominal-identity and overlay-definition proofs. No blind dispatch was restored.

`acceptance.txt` records the exact required target command exiting 0: 38 tests pass,
including all aggregate owner/capture repairs, ordinary exact-owner conversions,
Wrapper and trait-dispatch counterexamples, full self types, literals, lifetimes,
recursion, selected cursors, refusal controls and effectful-call construction.
`orchestration.txt` records the one directly affected positive fixture passing.

The acceptance command suppresses successful test stdout, so the two new tests were
replayed narrowly with `--nocapture` solely to retain the requested source/runtime
artifacts. `runtime.txt` records 16 transformed programs compiled and executed:
two comment layouts and both nested-reference orders, each in ordinary/forced
preview/apply. Comment outputs remain `3`; nested outputs remain `4`. Each plan
converts one call. Preview checks all original workspace bytes; application checks
the written source. These are scripted semantic responses plus real compiler and
runtime observations, not live-analyzer end-to-end claims.

## Gates and limitations

`format.txt`: scoped remote rustfmt completed with exit 0. Only the four changed
Rust files were formatted, with child traversal disabled.
`lint.txt`: strict scoped clippy failed, exit 101, with 103 diagnostics.
`lint-comparison.json` compares error messages, primary paths and primary source
lines to the retained preceding lint run: no added or removed diagnostics.
That preceding run's comparison against the pinned base remains in
`../1316-aggregate-repair/lint-baseline-comparison.json`. This is a baseline lint
limit, not a clean strict-lint result; unrelated code was not changed.

`tool-failures.txt` records the unavailable MCP navigation call and successful CLI
fallback. Publication was forbidden by the task, so no issue or message was sent.
`commands.json` retains sanitized commands and actual terminal statuses.
`runtime-sha256.json` indexes exact source/stdout blocks in the runtime log.
`source-sha256.json` hashes changed Rust sources; `artifact-sha256.json` hashes
this evidence set. Node addresses and checkout/temporary paths are normalized;
fixture Rust source and runtime stdout are retained verbatim.
