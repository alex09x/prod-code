# Issue 1352: native Rust extensions

Base: `c8b8fd85beb439e73380b934bb43ccea59281b25`.

## Observed failure and expected behavior

The baseline native gateway returned JSON-RPC error `-32601` for both
`rust-analyzer/viewSyntaxTree` and `rust-analyzer/expandMacro`. This differs from the
original silent-timeout hypothesis: this base already has an unsupported-method
response in `session/lsp_rust.rs`. `baseline.txt` preserves both exact requests and
responses, including their string and numeric IDs. The baseline test exited 101.

Expected: the first method returns the analyzer's JSON syntax-tree string; the second
returns the actual macro name and expansion at the requested occurrence, or null when
no expansion is available. Invalid parameters must receive errors. Requests must see
their session's proposal, worktree, validation engine, and crate edition.

## Source inspection and implementation

The existing pinned analyzer is revision
`3abd2c9f653a27e8c1b19195b1582634a6fe86e1`. Before editing, its cached source was
inspected at `crates/ide/src/lib.rs`, `crates/ide/src/view_syntax_tree.rs`, and
`crates/ide/src/expand_macro.rs`.

- `Analysis::view_syntax_tree(FileId)` already returns the upstream JSON string.
  Its nodes/tokens carry uppercase kinds and `[byte offset, line, byte column]`
  coordinates. The native adapter returns this string unchanged.
- `Analysis::expand_macro(FilePosition)` resolves the selected macro semantically
  and returns `ExpandedMacro { name, expansion }`. The adapter returns those exact
  fields, including upstream whitespace and parentheses.
- Both methods enter through `EDITOR_METHODS` and actual gateway dispatch. They use
  the existing bounded query runner, which activates the session and selects its
  worktree snapshot, supports cancellation/retries, and bounds lock acquisition.
- Extension positions are checked against the exact snapshot text in UTF-16 units.
  Out-of-document coordinates, integer overflow, split surrogate pairs and CRLF
  delimiter positions are refused with `-32602`. Analyzer/deadline errors use
  `-32603`. Unavailable expansions return null. IDs are preserved; notifications
  do not produce invented IDs.
- Document-validation errors retain cancellation identity so the existing query
  runner can retry concurrent revisions.

No external analyzer was started or used to supply replies. No test-only exports,
dependency pins, root manifest changes or MCP changes were needed.

## Test design

`native_rust_extensions` loads real Cargo fixtures using `RustEngine::load`, obtains
session leases from `WorkspaceManager`, and calls `on_client_message` with real wire
payloads, path translation, session metadata, and the gateway's output queue. It does
not use ScriptedGateway or canned responses. Fixtures disable procedural macro
workers and build scripts; declarative macros still run through the analyzer.

Assertions cover:

- Real tree data, JSON string encoding, IDs, complete token reconstruction and exact
  byte/line/byte-column coordinates over Unicode and CRLF proposals.
- Two identically spelled macros in different scopes, with different real expansions.
- Alternating private session proposals, an untouched session, close/revert and disk
  source preservation.
- An attached worktree plus a distinct, real validation engine, two validation sessions,
  the main worktree and the base source.
- Rust 2021 versus 2024 `gen` syntax and Rust 2024 `expr` macro matching for a const block.
- Missing/invalid coordinates and documents, unresolved/non-macro positions,
  notifications/null IDs, and a deliberately held engine lock with a 10 ms server budget.

Each fixture holds a test-local semaphore so default parallel test execution cannot
load an unbounded number of sysroots. The receive bound is 65 seconds, allowing the
existing 60-second native semantic deadline to return an explicit error.

## Recorded experiments

Commands below used the assigned Linux build node. `$BUILD_NODE` substitutes for its
private address. Command outputs redact only private build/cache/fixture paths and
the node address; request/result content remains intact.

Common command prefix:

`prod-code exec --remote "$BUILD_NODE" --timeout-secs 900 --`

1. `cargo test -p prod-code-gateway --test native_rust_extensions native_extensions_answer_through_gateway_dispatch -- --nocapture`
   initially exited 101 because the new helper incorrectly treated rapidfire receive
   as Option and tried to Debug-print an opaque frame. Both compiler diagnostics are
   retained in `initial-test-compile.txt`; the helper was fixed before the baseline.
2. The same command then reproduced the real unsupported-method failure, exit 101
   (`baseline.txt`).
3. `cargo test -p prod-code-gateway --test native_rust_extensions -- --nocapture --test-threads=1`
   exited 101 because an initially chosen 3-second test deadline was too short for cold
   native semantic queries (`short-deadline.txt`). No production deadline was increased.
4. The baseline-test command with a longer receive bound exited 0. The first native
   tree took 19,646 ms; expansion then took 14 ms (`native-timing.txt`).
5. `cargo test -p prod-code-gateway --test native_rust_extensions -- --nocapture`
   passed four cases and failed only the edition test's expected text: actual
   `(const{7})`, expected `const{7}`, exit 101 (`development.txt`). Only the assertion
   was corrected to preserve the upstream result.

The final verification records and source hashes accompany this file. Parent review
and the pending refactor consumer compatibility check remain outside this worker's
scope. There was no daemon deployment/restart or publication.

## Final verification

The failed edition assertion was corrected and only that failed scope rerun:

`cargo test -p prod-code-gateway --test native_rust_extensions native_extensions_use_the_consumers_edition -- --nocapture`

Exit 0: one passed, four filtered out, 41.45 seconds (`edition.txt`). The production
implementation was unchanged. Tests were then organized into compact files.

Scoped formatting used the common remote prefix above, followed by:

```text
rustfmt --edition 2024 --config skip_children=true crates/prod-code-engine-rust/src/editor/mod.rs crates/prod-code-engine-rust/src/editor/extensions.rs crates/prod-code-engine-rust/src/editor/snapshot.rs crates/prod-code-gateway/src/lsp_handlers/mod.rs crates/prod-code-gateway/src/lsp_handlers/editor.rs crates/prod-code-gateway/src/lsp_handlers/native_extensions.rs crates/prod-code-gateway/tests/native_rust_extensions.rs crates/prod-code-gateway/tests/common/native_rust.rs crates/prod-code-gateway/tests/native_rust_extensions/views.rs
```

Exit 0, with formatted files copied back (`format.txt`). No workspace-wide formatter
or extra build/check was run.

`cargo clippy -p prod-code-engine-rust -p prod-code-gateway --lib --no-deps`

Exit 0, 20.60 seconds on the server. Non-denying lint reported one engine warning and
56 gateway warnings, all in untouched files; none in the new extension files
(`lint.txt`). No unrelated warning cleanup was performed.

Exact final acceptance suffix:

`cargo test -p prod-code-gateway --test native_rust_extensions`

Exit 0: five passed, zero failed, test runtime 155.31 seconds; server command runtime
160.2 seconds, test compilation 4.67 seconds, Linux x86_64 (`acceptance.txt`). This was
the single final compile/test gate after formatting and lint. It used default cargo
test concurrency; the test-local fixture semaphore bounded simultaneous engine loads.

`source-hashes.json` records SHA-256 of all nine final source files and the relevant
baseline sources. Later changes are evidence-only. Broader suites, procedural macros,
external analyzer behavior, and MCP refactor compatibility were not tested here.
