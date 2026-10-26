# Issue #1316 — raw identifier and array length owner boundaries

Acceptance base: `86fe4ee03ed9e20eeb57ce443d06620a139a46b6`.
The assigned checkout began clean. Historical evidence, including the retained
26-test run in `../1316-owner-header-followup/acceptance.txt`, is unchanged.

## Fresh reproductions before production changes

`before.txt` records the two public `header_leak` regressions, terminal exit 101.
Both original programs compile with rustc and print `8`. Preview and application
compile but print `16` in all four apply/force combinations for each defect.
The independent `Inner` implementation's `Self` becomes `Order`. These are new
executions in this checkout, separate from historical disposable reviewer probes.

The raw identifier fixture uses `Check { r#fn: 1 < 2 }`. The array fixture uses
`fn helper() -> [u8; 1 << 2] { [0; 4] }`. Both retain the outer owner's own Self
use and an independent zero-sized Inner owner. The before run fails after the
first array fixture, so the additional nested array/impl Trait control has only
after evidence. Original source, transformed source, rustc status and runtime
stdout are retained verbatim except for private address/path sanitization.

## Repair and verification

`crates/prod-code-mcp/src/move_method/context.rs:41` carries the raw-identifier
status through the fn keyword decision. Raw identifiers cannot start a header.
At line 70, square-bracket frames retain their entry header context. A direct
array semicolon enters an expression; comparison/shift operators there cannot
create generic delimiters. The closing bracket restores the surrounding type
context, including nested Option/tuple/impl Trait types. Existing opaque-token
and macro-token handling remains shared with ownership and rewriting scans.
There are no fixture-name exceptions or new refusal paths.

The two new tests start at
`crates/prod-code-mcp/tests/rust_method_preservation/owner_header_regressions.rs:14`
and line 23. They call the public move API with cursor-specific scripted gateway
answers and compile/execute the actual result. Every apply/force combination
uses a fresh workspace. Preview and any refusal check source, manifest and
sentinel bytes; application checks manifest and sentinel bytes. A refusal is a
test failure for these supported moves. No new refusal occurred. Existing
macro-owner refusal controls continue to check all three files unchanged.

`acceptance.txt` records terminal exit 0 on a Linux x86_64 build node: 30 passed,
0 failed. Both new regressions and the nested array/impl Trait control retain
runtime `8` in every mode. The entire existing target remains selected, retaining
the original method findings, preceding regressions and ownership/capture,
effect, dispatch, nominal-type, literal/comment and binding-cursor controls.
Successful stdout and serial test execution retain readable exact source/runtime
evidence without changing the Cargo test target or test filtering.

Changed-source rustfmt exited 0. The strict workspace format check exited 1;
normalized diagnostic bodies match retained evidence. Crate lint exited 1
(underlying clippy exit 101), reporting 210 errors. All 175 displayed diagnostic
bodies match retained evidence; 35 are omitted by the CLI, so this is not an
exhaustive clean-lint claim. See `verification-comparison.txt`. No failed check
is reported as passed or waived. No unrelated test target or broad suite ran.

`commands-and-failures.txt` retains exact sanitized commands and tool limitations.
`hashes.sha256` covers the final production/test files and all new evidence files
except itself. Changed source files are 209 and 141 lines, with existing standard
attribution headers preserved. No dependencies, polyglot code, installs,
publication, messages, other worktrees, delegation, background providers or manual
commits were involved. Task integration owns the result commit.

## Evidence limits

These are public API integration tests with scripted analyzer answers and actual
rustc/runtime verification, not a live-analyzer end-to-end run or macro expansion
proof. The repair is a bounded lexical header-context correction, not a complete
Rust parser. Strict baseline format/lint debt remains. Tool navigation failures
were recorded and worked around without publishing an issue, as required by the
task's no-publication scope.
