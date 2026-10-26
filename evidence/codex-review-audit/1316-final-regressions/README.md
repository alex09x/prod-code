# Issue #1316 — remaining Rust method regressions

Pinned and effective base: `61107a98e959d8d6517a1d67d5f84abcc5adc166`.
The worktree was clean at that commit. All earlier audit directories and their
failed evidence remain unchanged. Reviewer probes were treated as historical;
the new public reproductions below ran before any production edits.

## New public failures

`before-confirmed.txt` records a fresh execution of all three public-API
reproductions (terminal exit 101, three failed tests):

- An associated move from `Order(u64)` to unit `Tax`, returning
  `Option<impl Iterator<Item = usize>>`, compiles before and after but changes
  runtime output from `8` to `0`. The preview leaves the outer `Self` unchanged.
  A genuine nested `impl Inner` provides an independent-owner control.
- Ordinary `if !flag` and nested-function never-type blocks containing plain
  `"{self:?}"` data are refused. Separate sources distinguish the parallel owner
  and capture scanners. Normal and forced preview/application all refuse, with
  source, manifest and sentinel bytes checked unchanged after every refusal.
- For `mut t: Tax`, the cursor-specific gateway records a binding-reference
  query at `(4,19)` (inside `mut`) instead of `(4,21)` (the binding). The original
  compiles and prints `6`; the public preview leaves both `t.0` references and
  rustc rejects it with two `E0425` errors. The method call itself is rewritten.

`before-initial.txt` preserves the initial probe too. Its binding fixture used
an earlier module named `t` and an effectful receiver, producing `E0423` and an
unrewritten blocked call. The corrected fresh reproduction above uses ordinary
local receivers, isolates `E0425`, and keeps the existing earlier-module-name
tests unchanged. No compiler/runtime assertions were suppressed.

## Repairs

- `crates/prod-code-mcp/src/move_method/context.rs:16` follows lexical type/header
  context across nested generics, tuples, arrays, references and function types.
  Body/const blocks leave that context, so independent nested owners retain their
  `Self`. `syntax.rs` also rejects an unmatched generic closer when looking for
  an owner declaration, without confusing function-type arrows with closers.
- `crates/prod-code-mcp/src/move_method/context.rs:88` shares macro context between
  both scanners. A macro requires a preceding path and an immediately following
  token tree; intervening identifiers/literals/punctuation clear pending state.
  Comments remain trivia. `macro_rules! name { ... }` has an explicit name state.
  Unary negation and never types no longer mark ordinary blocks as macro input.
- `crates/prod-code-mcp/src/move_method/syntax.rs:133` locates the final binding
  token in the already validated simple pattern. It cannot select a substring
  of `mut`; lookup stays within the selected parameter, preserving earlier
  module-word protection.

The new public tests are in
`crates/prod-code-mcp/tests/rust_method_preservation/remaining_regressions.rs:14`,
`:73` and `:149`. The owner matrix includes Option, nested Result/Option, tuples,
array/tuple headers, nested associated opaque types and a reference with an
explicit lifetime. Typed comparisons precede the genuine nested-owner control.
Normal and forced previews preserve workspace bytes; previews and applications
are compiled and executed. The binding gateway answers only at the actual token
cursor, and final output records `(4,21)` and runtime `6` for both modes.

## Verification and limits

`after-focused.txt` records the initial coherent repair (three passed tests),
before the final array/nested-opaque/comparison controls were added. It is
historical focused evidence, not a claim about the final source hashes.

`acceptance.txt` is the final formatted source state: the full requested
`rust_method_preservation` target exited 0, **24 passed, 0 failed**, on a Linux
x86_64 build node. `RUST_TEST_NOCAPTURE=1` only exposes successful-test stdout;
the Cargo target and test selection are unchanged. The file contains exact
sanitized original/preview/applied source, rustc statuses and runtime output.
All inherited seven-issue fixes and normal moves, dispatch/type identity,
literal/comment/raw data, recursion, lifetimes, failed-reference refusal and
effectful-call controls remain in that target. No other test target ran.

Changed-file formatting exited 0. The mandatory workspace format check exited
1; its diagnostic bodies match inherited evidence after path/address/footer
normalization. Crate lint exited 1 (clippy 101), reporting 210 errors; all 175
displayed diagnostics match inherited evidence. The CLI omits 35 diagnostic
bodies, so exhaustive clean lint is not claimed. See
`verification-comparison.txt`, `format-check.txt` and `lint.txt`.

The tests use cursor-specific scripted gateway protocol answers and real rustc
and runtime execution, not a live analyzer. Existing conservative refusal of
ambiguous macro owners and changing implicit macro captures remains in place.
This is a bounded lexical repair, not a complete Rust parser or macro expander.

Changes are limited to the Rust move-method root modules, the requested test
target and this audit. No polyglot source, dependencies, installs, publication,
merges, other worktrees, delegation or background providers were involved.
Changed source modules are at most 396 lines. Task integration owns the result
commit; this worker made no commit. `hashes.sha256` covers every changed/new
file relative to the pinned base, except the manifest itself.
