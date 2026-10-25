# Issue 1330: escaped local dependency import paths

Pinned acceptance base: `f6063f7399342f220c93a2013e8e99be82749633`.
Repair input: `0a9c56a2878ab96aeea70c246b3032c0efc36b5d`.
Earlier audit logs and repairs remain unchanged. This pass addresses the blocking
`escaped_import_dependency_lost` review finding.

## Before and after

`before.txt` records the exact focused remote command (with private endpoint and
remote-copy arguments replaced by descriptive placeholders) and terminal exit 101.
Before production edits, an actual contained parent required and replaced its
Apple child module, then imported `example.org/\x61pple`. The detector returned
`None`, public checkout and compatible-default routing selected Linux, and the
Linux-only route succeeded: `(None, false, false, true)` instead of the expected
Apple constraint and Mac routing. All four observations were captured before the
test assertion, using the existing fixture and public routing helper.

`crates/prod-code-mcp/src/sync/engine/go_tokens.rs:66` decodes Go interpreted
string values before dependency matching: byte hex, three-digit octal, Unicode
escapes and standard simple escapes. Raw strings preserve backslashes and discard
carriage returns. Byte escapes are assembled before UTF-8 decoding. Rune literals
remain opaque and cannot introduce imports. Existing source byte limits bound
decoding; unchanged containment, ownership and traversal limits bound scanning.
The helper is shared by import and manifest tokenization, including quoted
single/block require and replace paths. No dependencies, external tree access,
network discovery or toolchain discovery were added to startup.

`crates/prod-code-mcp/src/cluster/tests/route_local_deps_review.rs:17` adds the
escaped import regression through detector, checkout placement, compatible-default
routing and Linux-only routing. Variants cover hex, octal, both Unicode escape
widths, aliases, import blocks, subpackages and raw carriage-return handling.
Separate cases preserve raw backslashes and exercise escaped manifest paths.

## Verification

- `format.txt`: one scoped remote rustfmt invocation, terminal exit 0.
- `lint.json`: strict crate lint returned CLI exit 1/compiler exit 101, with 210
  diagnostics identical to the retained complete record. None names a changed
  file. All 53 diagnostic files are byte-identical to the pinned base; their hashes
  are recorded. No baseline cleanup or redundant lint run was performed.
- `after.txt`: exact required remote cluster acceptance, terminal exit 0;
  72 passed, 0 failed, 1,363 filtered out. This includes the new regressions and
  earlier wrapper/workspace, nested module, independent child, replacement graph,
  engine, compatible-default, remembered-Mac alternative, OS-probe and pressure
  cases. No broader test suite or overlapping compile gate was run.
- `source-hashes.json`: red production/fixture and final source/evidence hashes.
  The red fixture was subsequently expanded with manifest coverage. Its final
  hash is intentionally different from the recorded red-run hash.
- `tool-failures.txt`: exact initial whitespace-check failure and resolution by
  the already planned formatter. Earlier failure logs remain untouched.

Formatting, lint, compilation and Rust tests executed on a real Linux build node.
Gateway platforms, engines and OS responses were deterministic loopback fakes;
contained go.mod/source files were real temporary fixtures. No real macOS
execution is claimed. All started commands reached terminal completion. No
topology/config changes, installs, restarts, deployments, publication, messages,
delegation or background providers were used. Changed source modules have fewer
than 400 lines and retain the standard attribution header.
