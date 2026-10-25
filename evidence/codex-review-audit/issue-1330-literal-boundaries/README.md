# Issue 1330: retain imports across literal bytes and comment boundaries

Pinned acceptance base: `f6063f7399342f220c93a2013e8e99be82749633`.
Repair input: `9b9822516a0aa7368958c03a636fd8fa8f386a9c`.
Earlier repairs and evidence are preserved. This pass completes the two blocking
findings `non_utf8_literal_erases_imports` and
`multiline_comment_loses_import_boundary`.

## Observed before production edits

`before.txt` records the exact focused command with the private endpoint replaced
by `<linux-build-node>`, and terminal exit 101. The three new tests failed.
The fixture contains a real parent go.mod requiring and locally replacing its
Apple child, a real child go.mod and unconditional macOS-only cgo source, plus an
unrelated independent child. No Go toolchain or dependency resolver is invoked.

With `import _ "example.org/apple"` followed by an ordinary `"\xff"` string,
the detector returned `None`, checkout and compatible-default routing selected
Linux, and Linux-only routing succeeded. The observed tuple was
`(None, false, false, true)` instead of the Apple constraint and Mac routes.
With `import _ "fmt" /*\n*/ var _ = "example.org/apple"`, the unused dependency
was scanned: the observed tuple was
`(Some(("apple/apple.go", "libproc.h")), false, false, false)` instead of
`(None, true, true, true)`. These are measured routing results, not static claims.

The first cause depended on treating every decoded ordinary string as UTF-8;
Go byte literals disprove that assumption. The second depended on treating a
multiline block comment as empty whitespace; Go's newline boundary disproves it.

## Repair and focused coverage

`crates/prod-code-mcp/src/sync/engine/go_tokens.rs:29` retains an undecodable
quoted value as an empty opaque quoted token, so it cannot match a module path
or erase earlier imports. Valid UTF-8 import and manifest paths still use Go
escape decoding. `go_tokens.rs:40` retains the newline in a multiline comment.

`crates/prod-code-mcp/src/sync/engine/local_go.rs:165` allows newlines following
the import keyword, which does not insert a semicolon. The parser consumes one
path per specification and requires a separator between block specifications
(`local_go.rs:191`). Subsequent declaration strings cannot become imports.

`crates/prod-code-mcp/src/cluster/tests/route_local_deps_literals.rs:14` covers
hex, octal and multiple arbitrary byte escapes following single/block imports.
The case at line 26 excludes strings in declarations following multiline
comments, for both ordinary and raw literals. The case at line 38 preserves
real dependencies across comments after the import keyword, comments between
block specifications, inline comments, and explicit semicolons. All ten fixture
variants exercise the detector and public checkout, compatible-default and
Linux-only routing via the retained routing helper.

Containment, ownership, module/source byte limits, graph traversal bounds and
local replacement resolution are unchanged. This remains a bounded in-checkout
dependency scan; it adds no external tree reads, network/toolchain discovery,
dependencies or startup subprocesses. All changed source modules are under
400 lines and retain the standard attribution header.

## Verification and execution limits

- `format.txt`: one scoped remote rustfmt invocation, terminal exit 0.
- `lint.txt`: the scoped crate lint command returned CLI exit 1/compiler exit
  101. `lint-summary.json` confirms all 210 diagnostics are identical to the
  retained previous run, no changed file has a diagnostic, and every diagnostic
  file is byte-identical to the pinned base. Baseline lint is not claimed green.
- `after.txt`: the exact required remote cluster unit acceptance returned
  terminal exit 0: 75 passed, 0 failed, 1,363 filtered out. All new regressions
  passed. Existing wrapper/go.work, independent/nested module, local replacement,
  required-engine, compatible-default, remembered-Mac alternative, OS-probe detail
  and pressure controls also passed. This was the sole final compile/test gate;
  no broader suite or duplicate successful test command was run.
- `source-hashes.json`: production and fixture hashes before the repair, final
  source/evidence hashes, and confirmation that earlier audit files are unchanged.
- `tool-failures.txt`: the initial discovery hook rejection and the ambiguous
  navigation response, with exact attempted commands and bounded fallbacks.

Rust formatting, lint, compilation and tests executed on a real Linux build
node. Gateway status/platform/engine responses were deterministic loopback fakes,
and Go files were actual temporary contained fixtures. No real macOS execution
is claimed. All started commands reached terminal completion. No topology or
config changes, installs, restarts, deployments, publication, messages,
delegation or background providers were used. Result commit creation belongs to
the parent task runner; this worker did not commit or publish.
